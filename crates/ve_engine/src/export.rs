//! Export: render every frame of the active sequence, mix its audio, encode.
//!
//! Runs on its own thread with its own compositor on the shared GPU device,
//! and its own decoders: sharing the preview's would make the two fight over
//! where each source is positioned. Frames are waited for (exact, never
//! "nearest") and a frame that cannot be made fails the export rather than
//! leaving a hole; audio is mixed offline, sample-accurately, frame by frame,
//! so picture and sound cannot drift.

use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::{Arc, Mutex};
use std::time::Duration;

use ve_media::{Mixer, VideoPool};
use ve_model::{MediaRef, Snapshot};
use ve_plugin_host::Registry;
use ve_ports::{AudioBlock, ColorTags, EncoderSettings, FrameData, PixelFormat, Platform, VideoFrame};
use ve_render::{Compositor, Quality};
use ve_time::Time;

/// What an export is encoded as. Video is Rec.709 at the sequence's size
/// and rate; audio is AAC, 48 kHz stereo.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ExportPreset {
    /// H.264, about 20 Mb/s at 1080p: plays everywhere.
    H264Mp4,
    /// H.264, about 50 Mb/s at 1080p: for uploading to sites that encode
    /// again (each generation loses less).
    H264Mp4High,
    /// HEVC, 10-bit where the encoder can, about 12 Mb/s at 1080p.
    HevcMp4,
    /// HEVC, about 30 Mb/s at 1080p.
    HevcMp4High,
    /// ProRes 422 Proxy: offline edits, reviews.
    ProResProxy,
    /// ProRes 422 LT.
    ProResLt,
    /// ProRes 422.
    ProRes422,
    /// ProRes 422 HQ: the master.
    ProResMov,
    /// ProRes 4444: full-resolution colour (4:4:4) for grading and effects
    /// work elsewhere.
    ProRes4444,
    /// A proxy: small H.264 for smooth previews (not offered as an export).
    Proxy,
}

impl ExportPreset {
    /// The presets offered for export, in the order shown.
    pub const ALL: [ExportPreset; 9] = [
        ExportPreset::H264Mp4,
        ExportPreset::H264Mp4High,
        ExportPreset::HevcMp4,
        ExportPreset::HevcMp4High,
        ExportPreset::ProResProxy,
        ExportPreset::ProResLt,
        ExportPreset::ProRes422,
        ExportPreset::ProResMov,
        ExportPreset::ProRes4444,
    ];

    pub fn label(self) -> &'static str {
        match self {
            ExportPreset::H264Mp4 => "H.264 (MP4)",
            ExportPreset::H264Mp4High => "H.264, high bitrate (MP4)",
            ExportPreset::HevcMp4 => "HEVC (MP4)",
            ExportPreset::HevcMp4High => "HEVC, high bitrate (MP4)",
            ExportPreset::ProResProxy => "Apple ProRes 422 Proxy (MOV)",
            ExportPreset::ProResLt => "Apple ProRes 422 LT (MOV)",
            ExportPreset::ProRes422 => "Apple ProRes 422 (MOV)",
            ExportPreset::ProResMov => "Apple ProRes 422 HQ (MOV)",
            ExportPreset::ProRes4444 => "Apple ProRes 4444 (MOV)",
            ExportPreset::Proxy => "Proxy (H.264)",
        }
    }

    pub fn extension(self) -> &'static str {
        match self.codec() {
            "prores" => "mov",
            _ => "mp4",
        }
    }

    fn codec(self) -> &'static str {
        match self {
            ExportPreset::H264Mp4 | ExportPreset::H264Mp4High | ExportPreset::Proxy => "h264",
            ExportPreset::HevcMp4 | ExportPreset::HevcMp4High => "hevc",
            ExportPreset::ProResProxy | ExportPreset::ProResLt | ExportPreset::ProRes422 | ExportPreset::ProResMov | ExportPreset::ProRes4444 => {
                "prores"
            }
        }
    }

    /// The codec profile (ProRes' flavour); `None` for the codec's default.
    fn profile(self) -> Option<&'static str> {
        match self {
            ExportPreset::ProResProxy => Some("proxy"),
            ExportPreset::ProResLt => Some("lt"),
            ExportPreset::ProRes422 => Some("standard"),
            ExportPreset::ProResMov => Some("hq"),
            ExportPreset::ProRes4444 => Some("4444"),
            _ => None,
        }
    }

    /// The video bitrate for a `width`×`height` picture at `fps`, for the
    /// codecs that take one (ProRes sets its own by profile).
    pub fn video_bitrate(self, width: u32, height: u32, fps: f64) -> Option<u64> {
        let pixels = width as f64 * height as f64;
        let per_pixel = match self {
            ExportPreset::H264Mp4 => 10.0,
            ExportPreset::H264Mp4High => 24.0,
            ExportPreset::HevcMp4 => 6.0,
            ExportPreset::HevcMp4High => 15.0,
            // Proxies: about 0.12 bits a pixel — light to decode, fine to cut with.
            ExportPreset::Proxy => return Some((pixels * fps * 0.12) as u64),
            _ => return None,
        };
        Some((pixels * per_pixel) as u64)
    }

    /// One line for the export dialog: what this preset makes of a
    /// `width`×`height` sequence at `fps`.
    pub fn summary(self, width: u32, height: u32, fps: f64) -> String {
        if let Some(b) = self.video_bitrate(width, height, fps) {
            let depth = if self.codec() == "hevc" { "10-bit where this machine's encoder can, else 8-bit" } else { "8-bit" };
            return format!("About {:.0} Mb/s, {depth} 4:2:0.", b as f64 / 1e6);
        }
        match self {
            ExportPreset::ProRes4444 => "10-bit 4:4:4, intra-frame: large files.".into(),
            _ => "10-bit 4:2:2, intra-frame: large files, light to edit.".into(),
        }
    }
}

/// A running export, shared with whoever shows its progress.
pub struct ExportState {
    pub name: String,
    /// The first sequence frame exported (the in point's).
    pub first: i64,
    pub total: u64,
    done: AtomicU64,
    cancel: AtomicBool,
    /// Paused by the user, or held while the editor plays (the job system):
    /// the export waits between frames, using nothing.
    paused: AtomicBool,
    held: AtomicBool,
    gate: Mutex<()>,
    wake: std::sync::Condvar,
    result: Mutex<Option<Result<(), String>>>,
}

impl ExportState {
    pub fn progress(&self) -> f32 {
        self.done.load(Ordering::Relaxed) as f32 / self.total.max(1) as f32
    }

    pub fn cancel(&self) {
        self.signal(&self.cancel, true);
    }

    pub fn set_paused(&self, on: bool) {
        self.signal(&self.paused, on);
    }

    /// Hold the export while the editor plays back (resumes when it stops).
    pub fn set_held(&self, on: bool) {
        self.signal(&self.held, on);
    }

    /// Set a flag the export thread waits on. Under the gate's lock: set
    /// between the waiter's check and its sleep, the wake would be lost and
    /// a resumed export would sleep on for good.
    fn signal(&self, flag: &AtomicBool, on: bool) {
        let _g = self.gate.lock().unwrap();
        flag.store(on, Ordering::Relaxed);
        self.wake.notify_all();
    }

    pub fn is_paused(&self) -> bool {
        self.paused.load(Ordering::Relaxed) || self.held.load(Ordering::Relaxed)
    }

    /// Between frames: sleep (no timer) while paused or held.
    fn gate(&self) {
        let g = self.gate.lock().unwrap();
        let _g = self.wake.wait_while(g, |_| self.is_paused() && !self.cancel.load(Ordering::Relaxed)).unwrap();
    }

    /// Block until the export has finished (or failed, or was cancelled).
    pub fn wait(&self) -> Result<(), String> {
        let mut g = self.gate.lock().unwrap();
        loop {
            if let Some(r) = self.result() {
                return r;
            }
            g = self.wake.wait(g).unwrap();
        }
    }

    /// `Some` once finished (or failed, or cancelled).
    pub fn result(&self) -> Option<Result<(), String>> {
        self.result.lock().unwrap().clone()
    }
}

const SAMPLE_RATE: u32 = 48_000;

/// Decoder sessions per source. Measured (`ve bench --jobs`, M4 Pro, UHD
/// H.264 through VideoToolbox): one session 128 fps, three 335 fps.
const DECODE_LANES: usize = 3;

/// How long to wait for one decoded frame before giving up.
const FRAME_TIMEOUT: Duration = Duration::from_secs(20);

impl ExportState {
    /// The export of `project`'s active sequence to `out`, not started yet:
    /// whoever starts it can pause, hold or cancel it first.
    pub(crate) fn new(project: &Snapshot, platform: &Platform, out: &MediaRef) -> Arc<ExportState> {
        let seq = project.active();
        let rate = seq.map(|s| s.format.rate).unwrap_or(ve_time::Rate::FPS_24);
        // The in/out range when set, else the whole sequence: every frame that
        // touches it.
        let range = seq.map(|s| s.marks.range(s.duration())).unwrap_or_default();
        let first = range.start.to_frame(rate);
        let total = if range.duration > Time::ZERO { ((range.end() - Time(1)).to_frame(rate) - first + 1) as u64 } else { 0 };
        Arc::new(ExportState {
            name: platform.storage.display_name(out),
            first,
            total,
            done: AtomicU64::new(0),
            cancel: AtomicBool::new(false),
            paused: AtomicBool::new(false),
            held: AtomicBool::new(false),
            gate: Mutex::new(()),
            wake: std::sync::Condvar::new(),
            result: Mutex::new(None),
        })
    }
}

/// Start exporting `project`'s active sequence to `out`.
pub fn start(
    project: Snapshot,
    platform: Platform,
    registry: Arc<Registry>,
    gpu: (wgpu::Device, wgpu::Queue),
    preset: ExportPreset,
    out: MediaRef,
    importer: Option<Arc<dyn ve_render::TextureImporter>>,
) -> Arc<ExportState> {
    let state = ExportState::new(&project, &platform, &out);
    launch(state.clone(), project, platform, registry, gpu, preset, out, importer, Box::new(|| {}));
    state
}

/// Run `state`'s export on its own thread; `done` is called once its
/// result is set.
#[allow(clippy::too_many_arguments)]
pub(crate) fn launch(
    st: Arc<ExportState>,
    project: Snapshot,
    platform: Platform,
    registry: Arc<Registry>,
    gpu: (wgpu::Device, wgpu::Queue),
    preset: ExportPreset,
    out: MediaRef,
    importer: Option<Arc<dyn ve_render::TextureImporter>>,
    done: Box<dyn FnOnce() + Send>,
) {
    std::thread::Builder::new()
        .name("ve-export".into())
        .spawn(move || {
            let memory = platform.capabilities().memory_budget as usize;
            let pool = VideoPool::new(platform.storage.clone(), platform.media.clone(), memory / 8);
            // Several decoder sessions per source, a span each: a hardware
            // decoder is limited by the time each frame takes, not by the
            // hardware. The spans they decode ahead need memory to run side
            // by side; export is the one thing running, so half the budget.
            pool.set_lanes(DECODE_LANES, memory / 2);
            pool.set_gpu_frames(importer.is_some());
            // A panic in here (a GPU validation error, a codec bug) must still
            // end the export, or whoever shows its progress waits forever.
            let r = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| run(&project, &platform, &registry, &pool, &gpu, preset, &out, &st, importer)))
                .unwrap_or_else(|p| {
                    let why = p.downcast_ref::<String>().cloned().or_else(|| p.downcast_ref::<&str>().map(|s| s.to_string()));
                    Err(format!("export failed unexpectedly: {}", why.unwrap_or_default()))
                });
            drop(pool);
            if r.is_err() || st.cancel.load(Ordering::Relaxed) {
                // Leave no half-written file behind.
                if let Ok(res) = platform.storage.resolve_new(&out) {
                    if let Some(p) = res.path {
                        let _ = std::fs::remove_file(p);
                    }
                }
            }
            let r = if st.cancel.load(Ordering::Relaxed) { Err("cancelled".into()) } else { r };
            *st.result.lock().unwrap() = Some(r);
            {
                let _g = st.gate.lock().unwrap();
                st.wake.notify_all();
            }
            done();
        })
        .expect("export thread");
}

#[allow(clippy::too_many_arguments)]
fn run(
    project: &Snapshot,
    platform: &Platform,
    registry: &Registry,
    pool: &Arc<VideoPool>,
    (device, queue): &(wgpu::Device, wgpu::Queue),
    preset: ExportPreset,
    out: &MediaRef,
    state: &ExportState,
    importer: Option<Arc<dyn ve_render::TextureImporter>>,
) -> Result<(), String> {
    let seq = project.active().cloned().ok_or("no sequence to export")?;
    if state.total == 0 {
        return Err("the sequence is empty".into());
    }
    let _awake = platform.system.begin_background_task("Exporting");
    let f = &seq.format;
    let settings = EncoderSettings {
        video_codec: preset.codec().into(),
        audio_codec: "aac".into(),
        container: preset.extension().into(),
        width: f.width,
        height: f.height,
        rate: f.rate,
        sample_rate: SAMPLE_RATE,
        channels: 2,
        video_bitrate: preset.video_bitrate(f.width, f.height, f.rate.as_f64()),
        profile: preset.profile().map(Into::into),
        prefer_hardware: true,
    };
    let target = platform.storage.resolve_new(out).map_err(|e| e.to_string())?;
    let mut enc = match platform.media.open_encoder(&target, &settings) {
        Ok(enc) => enc,
        // A proxy only has to play here: where no H.264 encoder exists (an
        // LGPL build has no x264; no GPU encoder), MPEG-4 Part 2 does.
        Err(ve_ports::MediaError::Unsupported(_)) if preset == ExportPreset::Proxy => {
            let mpeg4 = EncoderSettings { video_codec: "mpeg4".into(), prefer_hardware: false, ..settings.clone() };
            platform.media.open_encoder(&target, &mpeg4).map_err(|e| e.to_string())?
        }
        Err(e) => return Err(e.to_string()),
    };
    let mut comp = Compositor::new(device);
    comp.set_importer(importer);
    // 16 bits per channel out of the compositor: 10-bit codecs get 10 real
    // bits, 8-bit ones are rounded from the full picture, not the monitor's.
    comp.set_deep_output(true);
    let mut mixer = Mixer::new(platform.storage.clone(), platform.media.clone(), SAMPLE_RATE);
    if let Some(p) = registry.audio_plugins() {
        mixer.set_plugins(p);
    }
    let samples_at = |t: Time| (t.ticks() as i128 * SAMPLE_RATE as i128 / ve_time::TICKS_PER_SECOND as i128) as usize;
    let mut audio = Vec::new();
    let frame_duration = f.rate.frame_duration();
    let encode = |enc: &mut dyn ve_ports::Encoder, rb: ve_render::Readback, at: Time| -> Result<(), String> {
        let bpp = rb.bytes_per_pixel() as usize;
        let (w, h, px) = rb.finish(device).ok_or("could not read the rendered frame")?;
        enc.push_video(VideoFrame {
            pts: at,
            duration: frame_duration,
            width: w,
            height: h,
            format: if bpp == 8 { PixelFormat::Rgba16 } else { PixelFormat::Rgba8 },
            color: ColorTags::default(),
            data: FrameData::Cpu { planes: vec![px], strides: vec![w as usize * bpp] },
        })
        .map_err(|e| e.to_string())
    };
    let mut pending: Option<(ve_render::Readback, Time)> = None;
    for i in 0..state.total {
        state.gate();
        if state.cancel.load(Ordering::Relaxed) {
            return Err("cancelled".into());
        }
        let t = f.rate.frame_to_time(state.first + i as i64);
        let plan = ve_render::evaluate(&seq, t, Quality::FULL);
        crate::frame::prefetch(project, t, Time::from_seconds(1), false, pool);
        let frame = crate::frame::resolve(project, plan, registry, pool, Some(FRAME_TIMEOUT));
        if let Some(why) = frame.missing.first() {
            let tc = ve_time::Timecode::from_time(t, f.rate, f.rate.is_drop_frame_rate());
            return Err(format!("frame {tc}: {why}"));
        }
        comp.render(device, queue, &frame.plan, &frame.layers, frame.seq_size, frame.space);
        // Pipelined: this frame's pixels come back while the next renders,
        // and the previous frame is encoded meanwhile.
        let rb = comp.start_readback(device, queue).ok_or("could not read the rendered frame")?;
        if let Some((prev, at)) = pending.replace((rb, t)) {
            encode(&mut *enc, prev, at)?;
        }
        // This frame's share of the audio, counted in whole samples from zero.
        let next = f.rate.frame_to_time(state.first + i as i64 + 1);
        let n = samples_at(next) - samples_at(t);
        audio.resize(n * 2, 0.0);
        mixer.render(project, &seq, t, &mut audio);
        enc.push_audio(AudioBlock { pts: t, sample_rate: SAMPLE_RATE, channels: 2, samples: audio.clone() }).map_err(|e| e.to_string())?;
        state.done.store(i + 1, Ordering::Relaxed);
    }
    if let Some((last, at)) = pending.take() {
        encode(&mut *enc, last, at)?;
    }
    enc.finish().map_err(|e| e.to_string())
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Every offered preset names a container its codec fits, and the dialog
    /// can describe it; ProRes takes no bitrate (its profile sets one).
    #[test]
    fn presets_are_consistent() {
        for p in ExportPreset::ALL {
            assert_eq!(p.extension(), if p.codec() == "prores" { "mov" } else { "mp4" }, "{p:?}");
            assert_eq!(p.video_bitrate(1920, 1080, 25.0).is_none(), p.codec() == "prores", "{p:?}");
            assert_eq!(p.profile().is_some(), p.codec() == "prores", "{p:?}");
            assert!(!p.summary(1920, 1080, 25.0).is_empty());
        }
        assert!(!ExportPreset::ALL.contains(&ExportPreset::Proxy), "proxies are not offered as an export");
        let labels: std::collections::HashSet<_> = ExportPreset::ALL.iter().map(|p| p.label()).collect();
        assert_eq!(labels.len(), ExportPreset::ALL.len(), "every preset has its own name");
        assert_eq!(ExportPreset::H264Mp4.summary(1920, 1080, 25.0), "About 21 Mb/s, 8-bit 4:2:0.");
    }
}
