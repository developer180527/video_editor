//! The iPadOS adapters.
//!
//! What iPadOS rules out, and how the engine copes (it asks
//! [`Capabilities`], never the OS name):
//! - **No child processes** → `Platform::processes` is `None`; process add-ons
//!   are unavailable and WASM add-ons take their place.
//! - **No loading code from files** → native plugins are linked into the app
//!   and listed by `linked()`; `load` refuses.
//! - **No JIT** → WASM add-ons are interpreted.
//! - **Sandboxed files** → media the user picks in the document picker is kept
//!   as a security-scoped bookmark. Phase B: `make_ref` creates the bookmark,
//!   `resolve` resolves it and holds the scope open in `Resolved::guard`.
//!   Until then files inside the app container work through `FileStorage`.
//! - **Tight memory** → a smaller cache budget; the OS's memory warnings are
//!   observed and reported through `System::memory_pressure`, which the
//!   frame caches (Phase C) evict on.
//!
//! Written so it also compiles on macOS, which keeps `cargo check` of the
//! whole workspace honest on any machine.

use std::sync::Arc;
use std::time::{Duration, Instant};
use platform_headless::{FileStorage, LinkedOnly};
use ve_ports::*;

#[cfg(target_os = "ios")]
mod audio_session;
#[cfg(target_os = "ios")]
mod memory;

pub struct IosSystem {
    start: Instant,
}

impl System for IosSystem {
    fn capabilities(&self) -> Capabilities {
        Capabilities {
            platform_name: "iPadOS",
            processes: false,
            dynamic_libraries: false,
            jit: false,
            memory_budget: 1 << 30,
            hw_decode: ["h264", "hevc", "prores"].map(String::from).to_vec(),
            hw_encode: ["h264", "hevc"].map(String::from).to_vec(),
            suspends_in_background: true,
        }
    }

    fn monotonic(&self) -> Duration {
        self.start.elapsed()
    }

    /// Critical for 10 s after the OS warns, then Warning for a minute.
    fn memory_pressure(&self) -> MemoryPressure {
        #[cfg(target_os = "ios")]
        if let Some(ago) = memory::since_last_warning() {
            return if ago < Duration::from_secs(10) {
                MemoryPressure::Critical
            } else if ago < Duration::from_secs(60) {
                MemoryPressure::Warning
            } else {
                MemoryPressure::Normal
            };
        }
        MemoryPressure::Normal
    }
}

/// Storage inside the app container. `HOME` is the container on iOS.
pub fn storage() -> FileStorage {
    let home = std::env::var("HOME").map(std::path::PathBuf::from).unwrap_or_else(|_| std::env::temp_dir());
    FileStorage {
        cache: home.join("Library/Caches"),
        app_data: home.join("Library/Application Support"),
        documents: home.join("Documents"),
    }
}

pub fn platform(media: Arc<dyn MediaBackend>, linked: Vec<fn() -> Box<dyn NativeLibrary>>) -> Platform {
    #[cfg(target_os = "ios")]
    {
        memory::observe();
        audio_session::configure();
    }
    Platform {
        system: Arc::new(IosSystem { start: Instant::now() }),
        storage: Arc::new(storage()),
        media,
        audio: Arc::new(audio_cpal::CpalAudio),
        libraries: Arc::new(LinkedOnly { make: linked }),
        processes: None,
    }
}
