//! Samples the playhead at 60 Hz, as the monitor does, and reports how even
//! the steps are — with and without extrapolating between audio callbacks.
use std::time::Duration;
use ve_playback::{Clocks, Transport};
use ve_ports::{clock_now, AudioOutput};

fn main() {
    let a = audio_cpal::CpalAudio;
    let cfg = a.preferred().expect("no device");
    let mut s = a.open(cfg, Box::new(|b: &mut [f32]| b.fill(0.0))).unwrap();
    s.play().unwrap();
    std::thread::sleep(Duration::from_millis(300));
    let clock = s.clock();
    let now = |stamped: bool| {
        let (f, at) = clock.read();
        let m = clock_now();
        Clocks { audio: Some((f, if stamped { at } else { m })), monotonic: m }
    };
    for stamped in [false, true] {
        let mut t = Transport::new(cfg.sample_rate);
        t.play(1.0, now(stamped));
        let mut last = t.position_at(now(stamped));
        let mut wall = clock_now();
        let mut steps = Vec::new();
        for _ in 0..120 {
            std::thread::sleep(Duration::from_micros(16_667));
            let p = t.position_at(now(stamped));
            let w = clock_now();
            // How far the playhead step differs from the time that passed.
            steps.push(((p - last).as_seconds_f64() - (w - wall).as_secs_f64()) * 1000.0);
            last = p;
            wall = w;
        }
        steps.sort_by(|a, b| a.partial_cmp(b).unwrap());
        println!(
            "{}: playhead error per frame, ms: min {:+.2} median {:+.2} max {:+.2}",
            if stamped { "extrapolated (new)" } else { "callback count only (old)" },
            steps[0],
            steps[60],
            steps[119]
        );
    }
}
