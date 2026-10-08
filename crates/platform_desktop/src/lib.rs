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
        let lib = unsafe { libloading::Library::new(path) }.map_err(|e| LibraryError::Other(e.to_string()))?;
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
pub fn platform(media: Arc<dyn MediaBackend>, linked: Vec<fn() -> Box<dyn NativeLibrary>>) -> Platform {
    Platform {
        system: Arc::new(DesktopSystem { start: Instant::now() }),
        storage: Arc::new(storage()),
        media,
        audio: Arc::new(audio_cpal::CpalAudio),
        libraries: Arc::new(DesktopLibraries { linked }),
        processes: Some(Arc::new(DesktopProcesses)),
    }
}
