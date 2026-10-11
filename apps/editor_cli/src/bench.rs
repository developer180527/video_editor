//! `ve bench`: the engine and compositor under load, measured the same way
//! every run, so a change can be judged by numbers.
//!
//! It makes its test media with the `ffmpeg` command-line tool, builds the
//! test projects (also saved as `.veproj`, to open in the app under
//! Instruments), then runs each scenario as the app would: the threaded
//! engine, the real audio device as the clock, and the GPU compositor
//! drawing what the Program monitor would show, at 60 Hz.
//!
//! For each it reports CPU time (all threads) per second of wall time — the
//! cores kept busy, the closest a process can get to its own energy — and
//! frames: drawn, late (shown before the decoders had the exact one), and
//! the time the compositor's GPU work took.

use std::path::{Path, PathBuf};
use std::process::Command;
use std::sync::Arc;
use std::time::{Duration, Instant};

use ve_engine::{edit, intrinsic, make_clip, make_generator_clip, Command as Cmd, Engine, EngineClient, Quality};
use ve_model::*;
use ve_render::Compositor;
use ve_time::Time;

/// Process CPU time (user + system, every thread) so far.
fn cpu_time() -> Duration {
    #[cfg(unix)]
    unsafe {
        let mut u: libc::rusage = std::mem::zeroed();
        libc::getrusage(libc::RUSAGE_SELF, &mut u);
        let tv = |t: libc::timeval| Duration::new(t.tv_sec as u64, t.tv_usec as u32 * 1000);
        tv(u.ru_utime) + tv(u.ru_stime)
    }
    #[cfg(not(unix))]
    Duration::ZERO
}

/// Peak resident memory, MB.
fn peak_mb() -> f64 {
    #[cfg(unix)]
    unsafe {
        let mut u: libc::rusage = std::mem::zeroed();
        libc::getrusage(libc::RUSAGE_SELF, &mut u);
        // Bytes on Apple, kilobytes elsewhere.
        let b = if cfg!(target_vendor = "apple") { u.ru_maxrss as f64 } else { u.ru_maxrss as f64 * 1024.0 };
        b / 1_048_576.0
    }
    #[cfg(not(unix))]
    0.0
}

fn ffmpeg(args: &[&str]) -> bool {
    Command::new("ffmpeg").args(["-y", "-loglevel", "error"]).args(args).status().is_ok_and(|s| s.success())
}

/// A clip with moving detail (codecs work as hard as on real footage) and
/// near-silent audio, `secs` long.
fn make(dir: &Path, name: &str, size: &str, rate: &str, secs: u32, video: &[&[&str]]) -> Option<PathBuf> {
    let out = dir.join(name);
    if out.exists() {
        return Some(out);
    }
    eprintln!("making {name}…");
    let src = format!("testsrc2=size={size}:rate={rate}:duration={secs}");
    let audio = format!("sine=frequency=440:sample_rate=48000:duration={secs},volume=0.01");
    // The first encoder that works here (hardware first where there is one).
    for v in video {
        let ok = ffmpeg(&[&["-f", "lavfi", "-i", &src, "-f", "lavfi", "-i", &audio], *v, &["-c:a", "aac", "-shortest", out.to_str()?]].concat());
        if ok {
            return Some(out);
        }
    }
    eprintln!("  could not make {name}");
    None
}

struct Media {
    hd: PathBuf,
    uhd_h264: PathBuf,
    uhd_hevc: PathBuf,
}

fn media(dir: &Path, secs: u32) -> Option<Media> {
    std::fs::create_dir_all(dir).ok()?;
    let h264 = ["-c:v", "libx264", "-preset", "veryfast", "-crf", "20", "-g", "60", "-pix_fmt", "yuv420p"];
    Some(Media {
        hd: make(dir, "hd_h264_30.mp4", "1920x1080", "30", secs, &[&h264])?,
        uhd_h264: make(dir, "uhd_h264_30.mp4", "3840x2160", "30", secs, &[&h264])?,
        uhd_hevc: make(dir, "uhd_hevc10_30.mov", "3840x2160", "30", secs, &[
            &["-c:v", "hevc_videotoolbox", "-profile:v", "main10", "-pix_fmt", "p010le", "-b:v", "60M", "-tag:v", "hvc1"],
            &["-c:v", "libx265", "-preset", "ultrafast", "-x265-params", "log-level=error", "-pix_fmt", "yuv420p10le", "-tag:v", "hvc1"],
        ])?,
    })
}

/// A test project: what is on the timeline.
#[derive(Clone, Copy, PartialEq)]
enum Kind {
    /// One clip on V1 (and its audio on A1).
    Plain,
    /// The clip with a blur, the grade and a glow; a title over it; a
    /// dissolve into a second copy half-way.
    Effects,
}

fn project(path: &Path, kind: Kind, secs: u32) -> Result<Engine, String> {
    let media = Arc::new(media_ffmpeg::Ffmpeg::new());
    let mut e = Engine::new(platform_desktop::platform(media, ve_builtins::linked()));
    e.new_project("bench");
    let asset = e.import(path.to_str().unwrap()).map_err(|x| x.to_string())?;
    let snap = e.snapshot();
    let seq = snap.active().unwrap().clone();
    let (v1, v2, a1) = (seq.tracks[0].id, seq.tracks[1].id, seq.tracks[2].id);
    let a = snap.assets[&asset].clone();
    let len = Time::from_seconds(secs as i64);
    let link = LinkId::new();
    let half = Time::from_seconds(secs as i64 / 2);
    let pieces: Vec<(Time, Time)> = if kind == Kind::Effects { vec![(Time::ZERO, half), (half, len - half)] } else { vec![(Time::ZERO, len)] };
    for (at, d) in pieces {
        let snap = e.snapshot();
        let items = [
            (v1, Arc::new(make_clip(e.plugins(), &seq.format, &a, TrackKind::Video, d, Some(link)))),
            (a1, Arc::new(make_clip(e.plugins(), &seq.format, &a, TrackKind::Audio, d, Some(link)))),
        ];
        e.execute(edit::overwrite(&snap, seq.id, at, &items).map_err(|x| x.to_string())?).map_err(|x| x.to_string())?;
    }
    if kind == Kind::Effects {
        let reg = e.plugins().clone();
        let snap = e.snapshot();
        let clips: Vec<ClipId> = snap.active().unwrap().tracks[0].clips.iter().map(|c| c.id).collect();
        for &clip in &clips {
            for (api, id, set) in [
                (PluginApi::Native, "org.ve.std.gaussian_blur", vec![("blurriness", Value::Float(12.0))]),
                (PluginApi::Builtin, intrinsic::GRADE, vec![("exposure", Value::Float(0.4)), ("saturation", Value::Float(120.0))]),
                (PluginApi::Native, "org.ve.std.glow", vec![]),
            ] {
                let info = reg.find_id(api, id).ok_or(format!("no effect {id}"))?;
                let mut params: OrdMap<String, Param> = info.params.iter().map(|p| (p.id.clone(), Param::Constant(ve_engine::default_value(&p.kind, p.default)))).collect();
                for (k, v) in set {
                    params.insert(k.into(), Param::Constant(v));
                }
                let effect = Effect { id: EffectId::new(), plugin: info.plugin.clone(), enabled: true, params };
                let index = e.snapshot().active().unwrap().tracks[0].clips.iter().find(|c| c.id == clip).unwrap().effects.len();
                e.execute(Cmd::AddEffect { clip, index, effect: Arc::new(effect) }).map_err(|x| x.to_string())?;
            }
        }
        // A dissolve on the cut.
        let one = Time::from_seconds(1);
        let tr = Transition { id: EffectId::new(), plugin: intrinsic::plugin_ref(intrinsic::DISSOLVE), before: one, after: one, params: OrdMap::new() };
        let snap = e.snapshot();
        e.execute(edit::set_transition(&snap, clips[1], ve_engine::Edge::Start, Some(tr)).map_err(|x| x.to_string())?).map_err(|x| x.to_string())?;
        // A title over everything.
        let title = make_generator_clip(e.plugins(), &seq.format, &intrinsic::plugin_ref(intrinsic::TITLE), len).ok_or("no title")?;
        let snap = e.snapshot();
        e.execute(edit::overwrite(&snap, seq.id, Time::ZERO, &[(v2, Arc::new(title))]).map_err(|x| x.to_string())?).map_err(|x| x.to_string())?;
    }
    Ok(e)
}

fn gpu() -> Option<(wgpu::Device, wgpu::Queue)> {
    let instance = wgpu::Instance::new(wgpu::InstanceDescriptor::new_without_display_handle());
    let adapter = pollster::block_on(instance.request_adapter(&wgpu::RequestAdapterOptions { power_preference: wgpu::PowerPreference::HighPerformance, ..Default::default() })).ok()?;
    let features = adapter.features() & wgpu::Features::TEXTURE_FORMAT_16BIT_NORM;
    let desc = wgpu::DeviceDescriptor { required_features: features, required_limits: adapter.limits(), ..Default::default() };
    pollster::block_on(adapter.request_device(&desc)).ok()
}

/// What a scenario measured.
struct Run {
    name: String,
    wall: Duration,
    cpu: Duration,
    drawn: u32,
    late: u32,
    gpu_ms: Vec<f64>,
    frame_ms: Vec<f64>,
}

impl Run {
    fn print(&self) {
        let pct = |v: &mut Vec<f64>, p: f64| {
            if v.is_empty() {
                return 0.0;
            }
            v.sort_by(f64::total_cmp);
            v[((v.len() - 1) as f64 * p).round() as usize]
        };
        let (mut g, mut f) = (self.gpu_ms.clone(), self.frame_ms.clone());
        let secs = self.wall.as_secs_f64();
        println!(
            "{:<34} {:>6.2} cores  {:>5.1} fps drawn  {:>4} late  frame p50 {:>5.1} p95 {:>5.1} ms  gpu p50 {:>5.1} p95 {:>5.1} ms",
            self.name,
            self.cpu.as_secs_f64() / secs,
            self.drawn as f64 / secs,
            self.late,
            pct(&mut f, 0.5),
            pct(&mut f, 0.95),
            pct(&mut g, 0.5),
            pct(&mut g, 0.95),
        );
    }
}

/// `secs` of the app's Program monitor loop: every 60 Hz tick, take the frame
/// at the playhead; draw it if it is new.
fn monitor(name: &str, client: &EngineClient, gpu: &(wgpu::Device, wgpu::Queue), quality: Quality, secs: f64, mut each: impl FnMut(&EngineClient, u32)) -> Run {
    let (device, queue) = gpu;
    let mut c = Compositor::new(device);
    c.set_importer(gpu_import::importer());
    let mut last: Option<(i64, bool)> = None;
    let rate = client.snapshot().active().map_or(ve_time::Rate::FPS_24, |s| s.format.rate);
    let mut run = Run { name: name.into(), wall: Duration::ZERO, cpu: Duration::ZERO, drawn: 0, late: 0, gpu_ms: vec![], frame_ms: vec![] };
    let (t0, c0) = (Instant::now(), cpu_time());
    let tick = Duration::from_secs_f64(1.0 / 60.0);
    let mut n = 0;
    while t0.elapsed().as_secs_f64() < secs {
        let next = t0 + tick * (n + 1);
        each(client, n);
        let start = Instant::now();
        if let Some(f) = client.frame(quality) {
            let key = (f.plan.time.to_frame(rate), f.complete);
            // The app draws when the picture changes (a new frame, or the
            // exact one arriving), not every tick.
            if last != Some(key) {
                last = Some(key);
                c.render(device, queue, &f.plan, &f.layers, f.seq_size, f.space);
                let g = Instant::now();
                let _ = device.poll(wgpu::PollType::wait_indefinitely());
                run.gpu_ms.push(g.elapsed().as_secs_f64() * 1000.0);
                run.drawn += 1;
                if !f.complete {
                    run.late += 1;
                }
                run.frame_ms.push(start.elapsed().as_secs_f64() * 1000.0);
            }
        }
        n += 1;
        if let Some(d) = next.checked_duration_since(Instant::now()) {
            std::thread::sleep(d);
        }
    }
    run.wall = t0.elapsed();
    run.cpu = cpu_time() - c0;
    run
}

/// Nothing asked of the engine at all: what it costs to sit there.
fn idle(name: &str, secs: f64) -> Run {
    let (t0, c0) = (Instant::now(), cpu_time());
    std::thread::sleep(Duration::from_secs_f64(secs));
    Run { name: name.into(), wall: t0.elapsed(), cpu: cpu_time() - c0, drawn: 0, late: 0, gpu_ms: vec![], frame_ms: vec![] }
}

pub fn bench(args: &[String]) -> i32 {
    let arg = |k: &str| args.iter().position(|a| a == k).and_then(|i| args.get(i + 1)).cloned();
    let dir = PathBuf::from(arg("--media").unwrap_or_else(|| "target/bench".into()));
    let secs: u32 = arg("--clip-secs").and_then(|s| s.parse().ok()).unwrap_or(30);
    let run_secs: f64 = arg("--secs").and_then(|s| s.parse().ok()).unwrap_or(10.0);
    let only = arg("--only");
    let Some(m) = media(&dir, secs) else {
        eprintln!("bench needs the ffmpeg command-line tool");
        return 1;
    };
    let Some(g) = gpu() else {
        eprintln!("no GPU");
        return 1;
    };
    println!("ve bench — {} s per scenario; cores = CPU time / wall time (all threads)", run_secs);
    let cases = [("1080p H.264", &m.hd, Kind::Plain), ("UHD H.264", &m.uhd_h264, Kind::Plain), ("UHD HEVC 10-bit", &m.uhd_hevc, Kind::Plain), ("UHD HEVC + effects", &m.uhd_hevc, Kind::Effects)];
    for (label, path, kind) in cases {
        if only.as_ref().is_some_and(|o| !label.to_lowercase().contains(&o.to_lowercase())) {
            continue;
        }
        let e = match project(path, kind, secs) {
            Ok(e) => e,
            Err(err) => {
                eprintln!("{label}: {err}");
                continue;
            }
        };
        let file = dir.join(format!("{}.veproj", label.to_lowercase().replace([' ', '.', '+'], "_").replace("__", "_")));
        let client = e.spawn(Arc::new(|| {}));
        client.save_as(MediaRef(format!("file:{}", std::path::absolute(&file).unwrap().display())));
        std::thread::sleep(Duration::from_millis(500));
        println!("\n{label}  ({})", file.display());
        idle("  idle (paused, nothing asked)", run_secs.min(5.0)).print();
        monitor("  paused, monitor open", &client, &g, Quality::FULL, run_secs.min(5.0), |_, _| {}).print();
        for (q, qn) in [(Quality { scale: 0.5, use_proxies: false }, "1/2"), (Quality::FULL, "full")] {
            client.seek(Time::ZERO);
            client.play(1.0);
            monitor(&format!("  playing at {qn}"), &client, &g, q, run_secs, |_, _| {}).print();
            client.stop();
        }
        // Scrubbing: a new place every other tick, as a dragged playhead does.
        let mut x = 7u64;
        monitor("  scrubbing at 1/2", &client, &g, Quality { scale: 0.5, use_proxies: false }, run_secs.min(5.0), |c, n| {
            if n % 2 == 0 {
                x = x.wrapping_mul(6364136223846793005).wrapping_add(1442695040888963407);
                c.seek(Time::from_seconds_f64((x >> 33) as f64 / (1u64 << 31) as f64 * secs as f64));
            }
        })
        .print();
        // Export: as fast as it goes (ProRes: a software encoder everywhere).
        let out = dir.join("bench_export.mov");
        let (t0, c0) = (Instant::now(), cpu_time());
        let job = client.export(g.clone(), ve_engine::ExportPreset::ProResMov, MediaRef(format!("file:{}", std::path::absolute(&out).unwrap().display())), gpu_import::importer());
        let r = loop {
            if let Some(r) = job.result() {
                break r;
            }
            std::thread::sleep(Duration::from_millis(20));
        };
        let wall = t0.elapsed().as_secs_f64();
        match r {
            Ok(()) => println!(
                "  {:<32} {:>6.2} cores  {:>5.1}× real time ({} s of video in {:.1} s)",
                "export ProRes",
                (cpu_time() - c0).as_secs_f64() / wall,
                secs as f64 / wall,
                secs,
                wall
            ),
            Err(err) => println!("  export failed: {err}"),
        }
    }
    println!("\npeak memory {:.0} MB", peak_mb());
    0
}
