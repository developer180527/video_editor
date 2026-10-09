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

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ExportPreset {
    H264Mp4,
    HevcMp4,
    ProResMov,
}

impl ExportPreset {
    pub const ALL: [ExportPreset; 3] = [ExportPreset::H264Mp4, ExportPreset::HevcMp4, ExportPreset::ProResMov];

    pub fn label(self) -> &'static str {
        match self {
            ExportPreset::H264Mp4 => "H.264 (MP4)",
            ExportPreset::HevcMp4 => "HEVC (MP4)",
            ExportPreset::ProResMov => "Apple ProRes 422 HQ (MOV)",
        }
    }

    pub fn extension(self) -> &'static str {
        match self {
            ExportPreset::ProResMov => "mov",
            _ => "mp4",
        }
    }

    fn codec(self) -> &'static str {
        match self {
            ExportPreset::H264Mp4 => "h264",
            ExportPreset::HevcMp4 => "hevc",
            ExportPreset::ProResMov => "prores",
        }
    }
}

/// A running export, shared with whoever shows its progress.
pub struct ExportState {
    pub name: String,
    pub total: u64,
    done: AtomicU64,
    cancel: AtomicBool,
    result: Mutex<Option<Result<(), String>>>,
}

impl ExportState {
    pub fn progress(&self) -> f32 {
        self.done.load(Ordering::Relaxed) as f32 / self.total.max(1) as f32
    }

    pub fn cancel(&self) {
        self.cancel.store(true, Ordering::Relaxed);
    }

    /// `Some` once finished (or failed, or cancelled).
    pub fn result(&self) -> Option<Result<(), String>> {
        self.result.lock().unwrap().clone()
    }
}

const SAMPLE_RATE: u32 = 48_000;

/// How long to wait for one decoded frame before giving up.
const FRAME_TIMEOUT: Duration = Duration::from_secs(20);

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
    let seq = project.active().cloned();
    let rate = seq.as_ref().map(|s| s.format.rate).unwrap_or(ve_time::Rate::FPS_24);
    let duration = seq.as_ref().map(|s| s.duration()).unwrap_or(Time::ZERO);
    // Every frame that starts before the end.
    let last = duration - Time(1);
    let total = if duration > Time::ZERO { last.to_frame(rate) as u64 + 1 } else { 0 };
    let state = Arc::new(ExportState {
        name: platform.storage.display_name(&out),
        total,
        done: AtomicU64::new(0),
        cancel: AtomicBool::new(false),
        result: Mutex::new(None),
    });
    let st = state.clone();
    std::thread::Builder::new()
        .name("ve-export".into())
        .spawn(move || {
            let budget = (platform.capabilities().memory_budget / 8) as usize;
            let pool = VideoPool::new(platform.storage.clone(), platform.media.clone(), budget);
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
        })
        .expect("export thread");
    state
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
        video_bitrate: None,
        prefer_hardware: true,
    };
    let target = platform.storage.resolve_new(out).map_err(|e| e.to_string())?;
    let mut enc = platform.media.open_encoder(&target, &settings).map_err(|e| e.to_string())?;
    let mut comp = Compositor::new(device);
    comp.set_importer(importer);
    // 16 bits per channel out of the compositor: 10-bit codecs get 10 real
    // bits, 8-bit ones are rounded from the full picture, not the monitor's.
    comp.set_deep_output(true);
    let mut mixer = Mixer::new(platform.storage.clone(), platform.media.clone(), SAMPLE_RATE);
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
        if state.cancel.load(Ordering::Relaxed) {
            return Err("cancelled".into());
        }
        let t = f.rate.frame_to_time(i as i64);
        let plan = ve_render::evaluate(&seq, t, Quality::FULL);
        crate::frame::prefetch(project, t, Time::from_seconds(1), pool);
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
        let next = f.rate.frame_to_time(i as i64 + 1);
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
