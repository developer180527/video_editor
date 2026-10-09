//! `ve` — the engine from a terminal.
//!
//!   ve probe <media>...        what a file contains
//!   ve new <project.veproj>    create an empty project
//!   ve info <project.veproj>   sequences, tracks, clips
//!   ve plugins                 every effect the engine loaded
//!   ve caps                    what this platform allows
//!   ve assemble <project> <media>...
//!                              a new project with the media end to end on V1/A1

use std::sync::Arc;
use ve_engine::Engine;
use ve_model::MediaRef;
use ve_time::Timecode;

fn engine() -> Engine {
    let media = Arc::new(media_ffmpeg::Ffmpeg::new());
    let mut e = Engine::new(platform_desktop::platform(media, ve_builtins::linked()));
    e.new_project("Untitled");
    e
}

fn file_ref(path: &str) -> MediaRef {
    let abs = std::path::absolute(path).expect("path");
    MediaRef(format!("file:{}", abs.display()))
}

fn main() {
    let args: Vec<String> = std::env::args().skip(1).collect();
    let code = match args.first().map(String::as_str) {
        Some("probe") if args.len() > 1 => probe(&args[1..]),
        Some("new") if args.len() == 2 => new(&args[1]),
        Some("info") if args.len() == 2 => info(&args[1]),
        Some("plugins") => plugins(),
        Some("caps") => caps(),
        Some("assemble") if args.len() > 2 => assemble(&args[1], &args[2..]),
        _ => {
            eprintln!("usage: ve probe <media>... | new <project> | info <project> | plugins | caps | assemble <project> <media>...");
            2
        }
    };
    std::process::exit(code);
}

fn probe(paths: &[String]) -> i32 {
    let mut e = engine();
    let mut code = 0;
    for p in paths {
        match e.import(p) {
            Ok(id) => {
                let snap = e.snapshot();
                let a = &snap.assets[&id];
                println!("{}", a.name);
                if let Some(i) = &a.info {
                    println!("  duration  {:.3} s", i.duration.as_seconds_f64());
                    if let Some(v) = &i.video {
                        println!("  video     {}x{} @ {}/{} fps, {}", v.width, v.height, v.rate.num, v.rate.den, v.codec);
                    }
                    for (k, au) in i.audio.iter().enumerate() {
                        println!("  audio {:<3} {} Hz, {} ({} ch), {}", k + 1, au.sample_rate, au.layout, au.channels, au.codec);
                    }
                }
            }
            Err(err) => {
                eprintln!("{p}: {err}");
                code = 1;
            }
        }
    }
    code
}

fn assemble(project: &str, media: &[String]) -> i32 {
    let mut e = engine();
    let mut at = ve_time::Time::ZERO;
    for m in media {
        let asset = match e.import(m) {
            Ok(a) => a,
            Err(err) => {
                eprintln!("{m}: {err}");
                return 1;
            }
        };
        let snap = e.snapshot();
        let seq = snap.active().unwrap().clone();
        let a = snap.assets[&asset].clone();
        let info = a.info.clone().unwrap();
        // Picture plus every audio stream, adding audio tracks as needed.
        let (adds, items) = ve_engine::clips_for_asset(e.plugins(), &seq, &a, info.duration, None, None);
        for add in adds {
            e.execute(add).expect("add track");
        }
        let snap = e.snapshot();
        let cmd = ve_engine::edit::overwrite(&snap, seq.id, at, &items).expect("place");
        e.execute(cmd).expect("place");
        at += info.duration;
    }
    match e.save_as(file_ref(project)) {
        Ok(()) => {
            println!("assembled {} clip(s) into {project}", media.len());
            0
        }
        Err(err) => {
            eprintln!("{project}: {err}");
            1
        }
    }
}

fn new(path: &str) -> i32 {
    let mut e = engine();
    match e.save_as(file_ref(path)) {
        Ok(()) => {
            println!("created {path}");
            0
        }
        Err(err) => {
            eprintln!("{path}: {err}");
            1
        }
    }
}

fn info(path: &str) -> i32 {
    let mut e = engine();
    if let Err(err) = e.open(file_ref(path)) {
        eprintln!("{path}: {err}");
        return 1;
    }
    let p = e.snapshot();
    println!("{} — {} asset(s), {} sequence(s)", p.name, p.assets.len(), p.sequences.len());
    for s in p.sequences.values() {
        let f = &s.format;
        println!("{}  {}x{} @ {:.3} fps, {}", s.name, f.width, f.height, f.rate.as_f64(), f.working_space);
        for t in &s.tracks {
            println!("  {:<4} {} clip(s)", t.name, t.clips.len());
            for c in &t.clips {
                let tc = |x| Timecode::from_time(x, f.rate, true);
                println!("       {}  {} → {}", c.name, tc(c.timeline_start), tc(c.timeline_range().end()));
            }
        }
    }
    0
}

fn plugins() -> i32 {
    let e = engine();
    for fx in e.plugins().effects() {
        println!("{:<28} {:?} v{}  {} ({} params{})", fx.plugin.id, fx.plugin.api, fx.plugin.major_version, fx.name,
            fx.params.len(), if fx.wgsl.is_some() { ", GPU" } else { "" });
    }
    0
}

fn caps() -> i32 {
    println!("{:#?}", engine().capabilities());
    0
}
