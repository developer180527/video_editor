//! Measures how the audio device's frame counter advances: the size and
//! spacing of its steps, which is what the playhead clock is built on.
use std::time::{Duration, Instant};
use ve_ports::AudioOutput;

fn main() {
    let a = audio_cpal::CpalAudio;
    let cfg = a.preferred().expect("no device");
    println!("device: {} Hz, {} ch", cfg.sample_rate, cfg.channels);
    let mut s = a.open(cfg, Box::new(|b: &mut [f32]| b.fill(0.0))).unwrap();
    s.play().unwrap();
    std::thread::sleep(Duration::from_millis(300));
    let start = Instant::now();
    let mut last = s.frames_played();
    let mut steps = Vec::new();
    let mut last_change = Instant::now();
    let mut gaps = Vec::new();
    while start.elapsed() < Duration::from_secs(2) {
        let f = s.frames_played();
        if f != last {
            steps.push(f - last);
            gaps.push(last_change.elapsed().as_secs_f64() * 1000.0);
            last_change = Instant::now();
            last = f;
        }
        std::thread::sleep(Duration::from_micros(200));
    }
    steps.sort();
    gaps.sort_by(|a, b| a.partial_cmp(b).unwrap());
    println!("{} steps in 2 s; step frames min {} median {} max {}", steps.len(), steps[0], steps[steps.len() / 2], steps[steps.len() - 1]);
    println!("time between steps ms: min {:.1} median {:.1} max {:.1}", gaps[0], gaps[gaps.len() / 2], gaps[gaps.len() - 1]);
}
