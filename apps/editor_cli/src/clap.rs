//! `ve clap [FILE.clap ...]` — load CLAP plugins (default: the platform's
//! plugin folders) as the editor does, and put every audio effect through
//! its paces: defaults, each parameter at its minimum, middle and maximum,
//! odd block sizes, reset, two instances at different rates, teardown.
//! Prints what each did to a test signal and flags NaNs, blow-ups and
//! failures. Exit status 1 when any effect failed.

use std::time::Instant;

use ve_engine::{AudioPlugins, ClapHost, ParamKind};

const RATE: u32 = 48_000;

fn as_refs(v: &[(String, f64)]) -> Vec<(&str, f64)> {
    v.iter().map(|(k, x)| (k.as_str(), *x)).collect()
}

/// A test signal: a -12 dBFS 440 Hz sine plus quiet noise, stereo (the
/// right side a little different, so channel swaps would show).
fn signal(n: usize, seed: &mut u32) -> (Vec<f32>, Vec<f32>) {
    let mut noise = || {
        *seed ^= *seed << 13;
        *seed ^= *seed >> 17;
        *seed ^= *seed << 5;
        (*seed as f32 / u32::MAX as f32 - 0.5) * 0.05
    };
    let l: Vec<f32> = (0..n).map(|i| (i as f32 * 440.0 * std::f32::consts::TAU / RATE as f32).sin() * 0.25 + noise()).collect();
    let r: Vec<f32> = (0..n).map(|i| (i as f32 * 660.0 * std::f32::consts::TAU / RATE as f32).sin() * 0.2 + noise()).collect();
    (l, r)
}

fn db(rms: f32) -> f32 {
    20.0 * rms.max(1e-9).log10()
}

fn rms(x: &[f32]) -> f32 {
    (x.iter().map(|v| v * v).sum::<f32>() / x.len().max(1) as f32).sqrt()
}

pub fn clap(args: &[String]) -> i32 {
    let libs = platform_desktop::DesktopLibraries { linked: vec![] };
    let t0 = Instant::now();
    let host = if args.is_empty() { ClapHost::scan(&libs) } else { ClapHost::scan_paths(&libs, args) };
    println!("scanned in {:.0} ms: {} effect(s)", t0.elapsed().as_secs_f64() * 1e3, host.effects().len());
    for (path, why) in &host.problems {
        println!("  ✗ {path}: {why}");
    }
    let mut failed = !host.problems.is_empty();
    let mut seed = 0x1234_5678u32;
    for info in host.effects() {
        println!("\n{} — {} ({}), {} parameter(s)", info.name, info.plugin.id, info.category, info.params.len());
        for p in info.params.iter().take(12) {
            let kind = match &p.kind {
                ParamKind::Choice(c) => format!("choice [{}]", c.join(" / ")),
                k => format!("{k:?} {}..{}", p.min, p.max),
            };
            println!("    {:>4}  {:<28} {kind}, default {}{}", p.id, p.label, p.default[0], if p.animatable { "" } else { ", not automatable" });
        }
        if info.params.len() > 12 {
            println!("    … and {} more", info.params.len() - 12);
        }
        let mut problems = Vec::new();
        let t = Instant::now();
        let mut fx = match host.instantiate(&info.plugin, RATE) {
            Ok(fx) => fx,
            Err(e) => {
                println!("  ✗ instantiate: {e}");
                failed = true;
                continue;
            }
        };
        let made = t.elapsed();
        let defaults: Vec<(String, f64)> = info.params.iter().map(|p| (p.id.clone(), p.default[0])).collect();
        let check = |l: &[f32], r: &[f32], what: &str, problems: &mut Vec<String>| {
            if l.iter().chain(r).any(|x| !x.is_finite()) {
                problems.push(format!("{what}: NaN or infinity"));
            }
            let peak = l.iter().chain(r).fold(0f32, |m, x| m.max(x.abs()));
            if peak > 64.0 {
                problems.push(format!("{what}: blew up (peak {peak:.1})"));
            }
        };

        // Defaults: 3 s in 512-frame blocks, timed.
        let n = RATE as usize * 3;
        let (mut l, mut r) = signal(n, &mut seed);
        let (in_l, in_r) = (rms(&l), rms(&r));
        let t = Instant::now();
        for (a, b) in l.chunks_mut(512).zip(r.chunks_mut(512)) {
            fx.process(&as_refs(&defaults), a, b);
        }
        let speed = 3.0 / t.elapsed().as_secs_f64();
        check(&l, &r, "defaults", &mut problems);
        let tail = n / 2..;
        println!(
            "  made in {:.0} ms · defaults: L {:+.1} dB, R {:+.1} dB · {:.0}× real time",
            made.as_secs_f64() * 1e3,
            db(rms(&l[tail.clone()])) - db(in_l),
            db(rms(&r[tail])) - db(in_r),
            speed
        );

        // Each parameter at its minimum, middle and maximum (0.25 s each).
        let mut swept = 0;
        for p in &info.params {
            let values = match &p.kind {
                ParamKind::Choice(c) => (0..c.len()).map(|i| i as f64).collect::<Vec<_>>(),
                _ => vec![p.min, (p.min + p.max) / 2.0, p.max],
            };
            for v in values {
                let mut params = defaults.clone();
                if let Some(x) = params.iter_mut().find(|(k, _)| *k == p.id) {
                    x.1 = v;
                }
                let (mut l, mut r) = signal(RATE as usize / 4, &mut seed);
                for (a, b) in l.chunks_mut(512).zip(r.chunks_mut(512)) {
                    fx.process(&as_refs(&params), a, b);
                }
                check(&l, &r, &format!("{} = {v}", p.label), &mut problems);
                swept += 1;
            }
            // Back to defaults, so one parameter's extreme does not colour the next.
            let (mut l, mut r) = signal(4096, &mut seed);
            fx.process(&as_refs(&defaults), &mut l, &mut r);
        }

        // Odd block sizes (a single frame, primes, more than the host's slice).
        for size in [1usize, 7, 333, 4096, 10_000] {
            let (mut l, mut r) = signal(size, &mut seed);
            fx.process(&as_refs(&defaults), &mut l, &mut r);
            check(&l, &r, &format!("block of {size}"), &mut problems);
        }
        // A jump: reset, then on.
        fx.reset();
        let (mut l, mut r) = signal(RATE as usize / 2, &mut seed);
        fx.process(&as_refs(&defaults), &mut l, &mut r);
        check(&l, &r, "after reset", &mut problems);

        // Two at once, at another rate, on other threads; then all torn down.
        let second = std::thread::scope(|s| {
            s.spawn(|| -> Result<(), String> {
                let mut b = host.instantiate(&info.plugin, 44_100)?;
                let (mut l, mut r) = signal(44_100, &mut 99u32.clone());
                b.process(&[], &mut l, &mut r);
                if l.iter().any(|x| !x.is_finite()) {
                    return Err("second instance at 44.1 kHz: NaN".into());
                }
                Ok(())
            })
            .join()
            .unwrap()
        });
        if let Err(e) = second {
            problems.push(e);
        }
        drop(fx);

        println!("  swept {swept} parameter setting(s), odd blocks, reset, a second instance at 44.1 kHz");
        if problems.is_empty() {
            println!("  ✓ ok");
        } else {
            failed = true;
            for p in &problems {
                println!("  ✗ {p}");
            }
        }
    }
    // Let the CLAP main thread finish destroying instances before exit.
    std::thread::sleep(std::time::Duration::from_millis(300));
    i32::from(failed)
}
