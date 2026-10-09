//! Links the FFmpeg built by `scripts/build_ffmpeg.sh` for the current target
//! and generates bindings from its headers.
//!
//! `FFMPEG_DIR` overrides the location (a prefix with `include/` and `lib/`).

use std::env;
use std::path::PathBuf;
use std::process::Command;

fn main() {
    let target = env::var("TARGET").unwrap();
    let root = PathBuf::from(env::var("CARGO_MANIFEST_DIR").unwrap()).join("../..");
    let dir = env::var("FFMPEG_DIR").map(PathBuf::from).unwrap_or_else(|_| {
        let name = match target.as_str() {
            "aarch64-apple-darwin" => "macos-arm64",
            "x86_64-apple-darwin" => "macos-x86_64",
            "aarch64-apple-ios" => "ios-arm64",
            "aarch64-apple-ios-sim" => "ios-sim-arm64",
            "x86_64-unknown-linux-gnu" => "linux-x86_64",
            "aarch64-unknown-linux-gnu" => "linux-arm64",
            "x86_64-pc-windows-msvc" => "windows-x86_64",
            "aarch64-pc-windows-msvc" => "windows-arm64",
            t => panic!("no FFmpeg build mapping for {t}: set FFMPEG_DIR or extend scripts/build_ffmpeg.sh"),
        };
        root.join("third_party/_build/ffmpeg").join(name)
    });
    println!("cargo:rerun-if-env-changed=FFMPEG_DIR");
    println!("cargo:rerun-if-changed={}", dir.join("lib").display());
    if !dir.join("include/libavformat/avformat.h").exists() {
        panic!(
            "FFmpeg not built at {}.\nRun: scripts/build_ffmpeg.sh <target> (see the script for the list)",
            dir.display()
        );
    }

    // Link line from FFmpeg's own pkg-config files, static.
    let pc = dir.join("lib/pkgconfig");
    let out = Command::new("pkg-config")
        .env("PKG_CONFIG_PATH", &pc)
        .env("PKG_CONFIG_LIBDIR", &pc)
        .args(["--static", "--libs", "libavformat", "libavcodec", "libswresample", "libswscale", "libavutil"])
        .output()
        .expect("pkg-config is needed to link FFmpeg");
    assert!(out.status.success(), "pkg-config failed: {}", String::from_utf8_lossy(&out.stderr));
    let flags = String::from_utf8(out.stdout).unwrap();
    let msvc = target.ends_with("-msvc");
    let out_dir = PathBuf::from(env::var("OUT_DIR").unwrap());
    if msvc {
        // FFmpeg's MSVC static libraries are named libavcodec.a; rustc asks
        // the linker for avcodec.lib. Same format, so copy them across.
        for l in ["avformat", "avcodec", "swresample", "swscale", "avutil"] {
            let a = dir.join(format!("lib/lib{l}.a"));
            if a.exists() {
                std::fs::copy(&a, out_dir.join(format!("{l}.lib"))).unwrap();
            }
        }
        println!("cargo:rustc-link-search=native={}", out_dir.display());
    }
    let mut words = flags.split_whitespace();
    let mut seen = std::collections::HashSet::new();
    while let Some(w) = words.next() {
        let line = if let Some(p) = w.strip_prefix("-L") {
            format!("cargo:rustc-link-search=native={}", native_path(p))
        } else if let Some(l) = w.strip_prefix("-l") {
            let kind = if l.starts_with("av") || l.starts_with("sw") { "static" } else { "dylib" };
            format!("cargo:rustc-link-lib={kind}={l}")
        } else if w == "-framework" {
            format!("cargo:rustc-link-lib=framework={}", words.next().unwrap())
        } else if let Some(l) = w.strip_suffix(".lib").filter(|l| !l.contains(['/', '\\'])) {
            // FFmpeg's MSVC build writes system libraries as `bcrypt.lib`,
            // not `-lbcrypt`; dropping them leaves BCrypt* and the Media
            // Foundation IIDs unresolved at link time.
            format!("cargo:rustc-link-lib=dylib={l}")
        } else {
            continue;
        };
        if seen.insert(line.clone()) {
            println!("{line}");
        }
    }

    // What FFmpeg's Windows code needs from the system whether or not its
    // .pc file names it: CNG for random seeds (libavutil), Media Foundation
    // and its interface IDs (the MF encoders), COM.
    if target.contains("windows") {
        for l in ["bcrypt", "mfplat", "mfuuid", "strmiids", "ole32", "user32"] {
            let line = format!("cargo:rustc-link-lib=dylib={l}");
            if seen.insert(line.clone()) {
                println!("{line}");
            }
        }
    }

    let mut b = bindgen::Builder::default()
        .header_contents(
            "ffmpeg.h",
            "#include <libavformat/avformat.h>\n#include <libavcodec/avcodec.h>\n\
             #include <libavutil/imgutils.h>\n#include <libswscale/swscale.h>\n\
             #include <libswresample/swresample.h>\n#include <libavutil/pixdesc.h>\n#include <libavutil/opt.h>\n#include <errno.h>\n",
        )
        .clang_arg(format!("-I{}", dir.join("include").display()))
        .allowlist_function("(av|avformat|avcodec|avio|sws|swr)_.*")
        .allowlist_type("(AV|Sws|Swr).*")
        .allowlist_var("(AV|FF_|LIBAV).*")
        .allowlist_var("EAGAIN")
        .default_enum_style(bindgen::EnumVariation::Rust { non_exhaustive: true })
        .layout_tests(false)
        .generate_comments(false);
    if target.contains("apple") {
        let sdk = if target.ends_with("ios-sim") {
            "iphonesimulator"
        } else if target.contains("ios") {
            "iphoneos"
        } else {
            "macosx"
        };
        let out = Command::new("xcrun").args(["--sdk", sdk, "--show-sdk-path"]).output().unwrap();
        b = b.clang_arg(format!("-isysroot{}", String::from_utf8(out.stdout).unwrap().trim()));
    }
    let bindings = b.generate().expect("bindgen over FFmpeg headers");
    bindings.write_to_file(PathBuf::from(env::var("OUT_DIR").unwrap()).join("ffmpeg.rs")).unwrap();
}

/// MSYS2's pkg-config speaks `/c/Users/...`; the MSVC linker wants
/// `C:/Users/...`. Other paths pass through.
fn native_path(p: &str) -> String {
    let b = p.as_bytes();
    if cfg!(windows) && b.len() >= 3 && b[0] == b'/' && b[1].is_ascii_alphabetic() && b[2] == b'/' {
        format!("{}:{}", (b[1] as char).to_ascii_uppercase(), &p[2..])
    } else {
        p.to_string()
    }
}
