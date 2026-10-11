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

/// How this platform makes the calling thread a background one: lower
/// priority, and on Apple silicon the efficiency cores. Set once by the
/// platform; workers that nobody waits on (thumbnails, waveforms) call
/// [`run_in_background`] when they start.
static BACKGROUND: std::sync::OnceLock<fn()> = std::sync::OnceLock::new();

/// Register the platform's way of lowering a thread (first call wins).
pub fn set_background_thread_hook(f: fn()) {
    let _ = BACKGROUND.set(f);
}

/// Make the calling thread a background thread, where the platform can.
pub fn run_in_background() {
    if let Some(f) = BACKGROUND.get() {
        f();
    }
}

/// What to do when the OS says memory is short (iOS's memory warning):
/// caches that can be rebuilt let go. Handlers are added once, by whoever
/// owns such caches (the engine).
static ON_MEMORY_WARNING: std::sync::Mutex<Vec<Box<dyn Fn() + Send + Sync>>> = std::sync::Mutex::new(Vec::new());

/// Run `f` whenever the platform reports a memory warning.
pub fn on_memory_warning(f: impl Fn() + Send + Sync + 'static) {
    ON_MEMORY_WARNING.lock().unwrap().push(Box::new(f));
}

/// The platform calls this when the OS warns that memory is short.
pub fn memory_warning() {
    for f in ON_MEMORY_WARNING.lock().unwrap().iter() {
        f();
    }
}

#[cfg(test)]
mod memory_tests {
    #[test]
    fn a_memory_warning_reaches_its_handlers() {
        use std::sync::atomic::{AtomicU32, Ordering};
        static HEARD: AtomicU32 = AtomicU32::new(0);
        super::on_memory_warning(|| {
            HEARD.fetch_add(1, Ordering::SeqCst);
        });
        super::memory_warning();
        assert!(HEARD.load(Ordering::SeqCst) >= 1);
    }
}
