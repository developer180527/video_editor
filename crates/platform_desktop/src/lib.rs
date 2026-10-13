//! The desktop adapters. Everything OS-specific about macOS, Windows and Linux
//! that the engine needs lives here (and only here).

use std::ffi::c_void;
use std::io::{Read, Write};
use std::path::PathBuf;
use std::process::{Child, ChildStdin, ChildStdout, Stdio};
use std::sync::Arc;
use std::time::{Duration, Instant};
use platform_headless::FileStorage;
use ve_ports::*;

const APP_DIR: &str = "VideoEditor";

pub struct DesktopSystem {
    start: Instant,
}

impl System for DesktopSystem {
    fn capabilities(&self) -> Capabilities {
        let mac = cfg!(target_os = "macos");
        Capabilities {
            platform_name: if mac { "macOS" } else if cfg!(windows) { "Windows" } else { "Linux" },
            processes: true,
            dynamic_libraries: true,
            jit: true,
            memory_budget: 4 << 30,
            // VideoToolbox on macOS. Windows (D3D11VA/NVDEC) and Linux (VAAPI)
            // are probed at run time in Phase C.
            hw_decode: if mac { ["h264", "hevc", "prores"].map(String::from).to_vec() } else { vec![] },
            hw_encode: if mac { ["h264", "hevc", "prores"].map(String::from).to_vec() } else { vec![] },
            suspends_in_background: false,
        }
    }

    fn monotonic(&self) -> Duration {
        self.start.elapsed()
    }
}

/// Storage in the OS's usual folders.
pub fn storage() -> FileStorage {
    let base = |d: Option<PathBuf>| d.unwrap_or_else(std::env::temp_dir).join(APP_DIR);
    FileStorage {
        cache: base(dirs::cache_dir()),
        app_data: base(dirs::data_dir()),
        documents: dirs::document_dir().unwrap_or_else(std::env::temp_dir),
    }
}

/// Plugin folders, by kind, in the places each ecosystem expects.
fn plugin_dirs(kind: &str) -> Vec<PathBuf> {
    let mut v = Vec::new();
    match kind {
        "ve" => {
            if let Some(d) = dirs::data_dir() {
                v.push(d.join(APP_DIR).join("Plugins"));
            }
        }
        "ofx" => {
            if cfg!(target_os = "macos") {
                v.push("/Library/OFX/Plugins".into());
            } else if cfg!(windows) {
                v.push("C:\\Program Files\\Common Files\\OFX\\Plugins".into());
            } else {
                v.push("/usr/OFX/Plugins".into());
            }
        }
        "clap" => {
            if cfg!(target_os = "macos") {
                v.push("/Library/Audio/Plug-Ins/CLAP".into());
                if let Some(h) = dirs::home_dir() {
                    v.push(h.join("Library/Audio/Plug-Ins/CLAP"));
                }
            } else if cfg!(windows) {
                v.push("C:\\Program Files\\Common Files\\CLAP".into());
            } else if let Some(h) = dirs::home_dir() {
                v.push(h.join(".clap"));
                v.push("/usr/lib/clap".into());
            }
        }
        _ => {}
    }
    v
}

fn plugin_extension(kind: &str) -> &'static str {
    match kind {
        "ofx" => "bundle", // Name.ofx.bundle
        "clap" => "clap",
        _ => "vep",
    }
}

/// Whether macOS has marked `path` as downloaded and unchecked (the
/// quarantine flag): loading it would be refused unless it is notarized.
#[cfg(target_os = "macos")]
fn quarantined(path: &std::path::Path) -> bool {
    use std::os::unix::ffi::OsStrExt;
    let Ok(c) = std::ffi::CString::new(path.as_os_str().as_bytes()) else { return false };
    let name = c"com.apple.quarantine";
    let n = unsafe { libc::getxattr(c.as_ptr(), name.as_ptr(), std::ptr::null_mut(), 0, 0, 0) };
    if n < 0 {
        return false;
    }
    // Notarized code passes despite the flag; ask the policy (quick, and
    // only for flagged files).
    std::process::Command::new("/usr/sbin/spctl")
        .args(["--assess", "--type", "open", "--context", "context:primary-signature"])
        .arg(path)
        .output()
        .map(|o| !o.status.success())
        .unwrap_or(false)
}

#[cfg(not(target_os = "macos"))]
fn quarantined(_: &std::path::Path) -> bool {
    false
}

/// The code inside a macOS bundle (`Name.clap/Contents/MacOS/Name`): the
/// executable its Info.plist names, else the only file there. `None` for a
/// plain file.
fn bundle_binary(path: &std::path::Path) -> Option<PathBuf> {
    if !path.is_dir() {
        return None;
    }
    let macos = path.join("Contents").join("MacOS");
    let plist = std::fs::read_to_string(path.join("Contents").join("Info.plist")).unwrap_or_default();
    let named = plist.split("<key>CFBundleExecutable</key>").nth(1).and_then(|rest| {
        let start = rest.find("<string>")? + "<string>".len();
        let end = rest[start..].find("</string>")?;
        Some(macos.join(rest[start..start + end].trim()))
    });
    named.filter(|p| p.is_file()).or_else(|| std::fs::read_dir(&macos).ok()?.filter_map(|e| e.ok()).map(|e| e.path()).find(|p| p.is_file()))
}

pub struct DesktopLibraries {
    /// Built-in plugins linked into the app.
    pub linked: Vec<fn() -> Box<dyn NativeLibrary>>,
}

struct Dylib {
    name: String,
    lib: libloading::Library,
}

impl NativeLibrary for Dylib {
    fn name(&self) -> &str {
        &self.name
    }
    fn symbol(&self, name: &str) -> Result<*const c_void, LibraryError> {
        unsafe {
            self.lib
                .get::<*const c_void>(name.as_bytes())
                .map(|s| *s)
                .map_err(|_| LibraryError::MissingSymbol(name.into()))
        }
    }
}

impl NativeLibraries for DesktopLibraries {
    fn linked(&self) -> Vec<Box<dyn NativeLibrary>> {
        self.linked.iter().map(|f| f()).collect()
    }

    fn discover(&self, kind: &str) -> Vec<String> {
        let ext = plugin_extension(kind);
        plugin_dirs(kind)
            .into_iter()
            .filter_map(|d| std::fs::read_dir(d).ok())
            .flatten()
            .filter_map(|e| e.ok().map(|e| e.path()))
            .filter(|p| p.extension().is_some_and(|x| x == ext))
            .map(|p| p.display().to_string())
            .collect()
    }

    fn load(&self, path: &str) -> Result<Box<dyn NativeLibrary>, LibraryError> {
        let file = bundle_binary(std::path::Path::new(path)).unwrap_or_else(|| path.into());
        // A plugin downloaded from the web and not notarized: macOS refuses
        // it, after seconds of checking. Say so at once, and how to allow it.
        if quarantined(std::path::Path::new(path)) {
            return Err(LibraryError::Other(format!(
                "macOS blocks this download (quarantined and not notarized). If you trust it, allow it with: xattr -dr com.apple.quarantine \"{path}\""
            )));
        }
        let lib = unsafe { libloading::Library::new(&file) }.map_err(|e| {
            // libloading puts dyld's reason in the error's source.
            let why = std::error::Error::source(&e).map(|s| s.to_string()).unwrap_or_default();
            LibraryError::Other(if why.is_empty() { e.to_string() } else { format!("{e}: {why}") })
        })?;
        Ok(Box::new(Dylib { name: path.into(), lib }))
    }
}

pub struct DesktopProcesses;

struct Proc {
    child: Child,
    stdin: ChildStdin,
    stdout: ChildStdout,
}

impl ChildProcess for Proc {
    fn stdin(&mut self) -> &mut (dyn Write + Send) {
        &mut self.stdin
    }
    fn stdout(&mut self) -> &mut (dyn Read + Send) {
        &mut self.stdout
    }
    fn try_wait(&mut self) -> Result<Option<i32>, ProcessError> {
        Ok(self.child.try_wait()?.map(|s| s.code().unwrap_or(-1)))
    }
    fn kill(&mut self) -> Result<(), ProcessError> {
        Ok(self.child.kill()?)
    }
}

impl ProcessHost for DesktopProcesses {
    fn spawn(&self, program: &str, args: &[String]) -> Result<Box<dyn ChildProcess>, ProcessError> {
        let mut child = std::process::Command::new(program)
            .args(args)
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .spawn()
            .map_err(|e| ProcessError::Spawn(program.into(), e.to_string()))?;
        let stdin = child.stdin.take().unwrap();
        let stdout = child.stdout.take().unwrap();
        Ok(Box::new(Proc { child, stdin, stdout }))
    }
}

/// The desktop platform. `media` is the decoding backend (FFmpeg in the app);
/// `linked` lists built-in plugins.
/// The calling thread becomes a background one (thumbnails, waveforms).
fn background_thread() {
    #[cfg(target_vendor = "apple")]
    {
        extern "C" {
            fn pthread_set_qos_class_self_np(qos: u32, relative_priority: i32) -> i32;
        }
        // QOS_CLASS_UTILITY: efficiency cores, out of the way of playback.
        unsafe { pthread_set_qos_class_self_np(0x11, 0) };
    }
    #[cfg(target_os = "linux")]
    unsafe {
        // Nice applies per thread on Linux.
        libc::setpriority(libc::PRIO_PROCESS, libc::gettid() as libc::id_t, 10);
    }
    #[cfg(windows)]
    {
        extern "system" {
            fn GetCurrentThread() -> *mut std::ffi::c_void;
            fn SetThreadPriority(thread: *mut std::ffi::c_void, priority: i32) -> i32;
        }
        // THREAD_MODE_BACKGROUND_BEGIN: low CPU, I/O and memory priority.
        unsafe { SetThreadPriority(GetCurrentThread(), 0x0001_0000) };
    }
}

pub fn platform(media: Arc<dyn MediaBackend>, linked: Vec<fn() -> Box<dyn NativeLibrary>>) -> Platform {
    ve_ports::set_background_thread_hook(background_thread);
    Platform {
        system: Arc::new(DesktopSystem { start: Instant::now() }),
        storage: Arc::new(storage()),
        media,
        audio: Arc::new(audio_cpal::CpalAudio),
        audio_in: Arc::new(audio_cpal::CpalInput),
        libraries: Arc::new(DesktopLibraries { linked }),
        processes: Some(Arc::new(DesktopProcesses)),
    }
}
