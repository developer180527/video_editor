//! Ports: everything the engine needs from outside itself, as traits.
//!
//! The engine (`ve_engine` and everything under it) is written against these
//! traits only. Each platform crate (`platform_desktop`, `platform_ios`,
//! `platform_headless`) implements them, and an app wires one set into a
//! [`Platform`] at start-up. **No crate above the adapters may use
//! OS `cfg`s** — `scripts/check_arch.sh` fails the build if one does.
//!
//! The engine asks [`System::capabilities`] what is possible instead of
//! asking which OS it is on: "can I start a process?" rather than "am I on
//! iPad?". A new platform is a new adapter, never a change to the engine.
//!
//! Windows, input, clipboard and the UI's GPU surface are **not** here: those
//! belong to the shell that hosts the UI (`platform_winit`). The engine runs
//! headless.

mod audio;
mod libraries;
mod media;
mod process;
mod storage;
mod system;

pub use audio::*;
pub use libraries::*;
pub use media::*;
pub use process::*;
pub use storage::*;
pub use system::*;

use std::sync::Arc;

/// One implementation of every port, chosen by the app at start-up.
#[derive(Clone)]
pub struct Platform {
    pub system: Arc<dyn System>,
    pub storage: Arc<dyn Storage>,
    pub media: Arc<dyn MediaBackend>,
    pub audio: Arc<dyn AudioOutput>,
    pub libraries: Arc<dyn NativeLibraries>,
    /// `None` where the OS forbids child processes (iPadOS).
    pub processes: Option<Arc<dyn ProcessHost>>,
}

impl Platform {
    pub fn capabilities(&self) -> Capabilities {
        self.system.capabilities()
    }
}
