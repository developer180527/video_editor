use std::time::Duration;

/// What this platform allows. The engine adapts to these, never to an OS name.
#[derive(Clone, Debug, PartialEq)]
pub struct Capabilities {
    /// For logs and bug reports only. Do not branch on it.
    pub platform_name: &'static str,
    /// Child processes can be started: tier-3 add-ons, out-of-process export.
    pub processes: bool,
    /// Native plugins can be loaded from files at run time (`dlopen`).
    /// False on iPadOS, where only plugins linked into the app exist.
    pub dynamic_libraries: bool,
    /// Generated machine code may run: WASM add-ons can be JIT-compiled.
    /// False on iPadOS; they are interpreted there.
    pub jit: bool,
    /// Bytes the engine's caches (frames, thumbnails, waveforms) may hold.
    pub memory_budget: u64,
    /// Hardware video decoders, by codec name ("h264", "hevc", "prores").
    pub hw_decode: Vec<String>,
    pub hw_encode: Vec<String>,
    /// The work may be suspended when the app is not in front (iPadOS).
    pub suspends_in_background: bool,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord)]
pub enum MemoryPressure {
    Normal,
    /// Shrink caches.
    Warning,
    /// Drop everything that can be rebuilt, now.
    Critical,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord)]
pub enum Thermal {
    Nominal,
    Fair,
    /// Lower preview quality.
    Serious,
    Critical,
}

/// Keeps the app running while it is held (an export when the user switches
/// apps). Dropping it ends the request.
pub trait BackgroundTask: Send {}

pub trait System: Send + Sync {
    fn capabilities(&self) -> Capabilities;
    fn memory_pressure(&self) -> MemoryPressure {
        MemoryPressure::Normal
    }
    fn thermal(&self) -> Thermal {
        Thermal::Nominal
    }
    /// Ask to keep running in the background. `None`: the platform does not
    /// suspend, or refused.
    fn begin_background_task(&self, _reason: &str) -> Option<Box<dyn BackgroundTask>> {
        None
    }
    /// Monotonic time since an arbitrary start. Not the playback clock: that
    /// is the audio device's.
    fn monotonic(&self) -> Duration;
}
