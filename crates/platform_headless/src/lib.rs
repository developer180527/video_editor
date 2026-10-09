//! Ports implemented with nothing but `std`.
//!
//! Used as-is by tests and the CLI, and as building blocks by the real
//! adapters ([`FileStorage`] serves every platform that has paths).

use std::fs::File;
use std::io::Write;
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};
use ve_model::MediaRef;
use ve_ports::*;

/// Media refs are `file:<absolute path>`.
pub struct FileStorage {
    pub cache: PathBuf,
    pub app_data: PathBuf,
    pub documents: PathBuf,
}

const SCHEME: &str = "file:";

impl FileStorage {
    /// All three locations under `root`.
    pub fn rooted(root: impl AsRef<Path>) -> Self {
        let r = root.as_ref();
        FileStorage { cache: r.join("cache"), app_data: r.join("data"), documents: r.join("documents") }
    }

    pub fn path_of(r: &MediaRef) -> Result<PathBuf, StorageError> {
        r.0.strip_prefix(SCHEME).map(PathBuf::from).ok_or_else(|| StorageError::NotFound(r.0.clone()))
    }
}

impl Storage for FileStorage {
    fn make_ref(&self, picked: &str) -> Result<MediaRef, StorageError> {
        let p = std::fs::canonicalize(picked.strip_prefix(SCHEME).unwrap_or(picked))
            .map_err(|_| StorageError::NotFound(picked.into()))?;
        Ok(MediaRef(format!("{SCHEME}{}", p.display())))
    }

    fn resolve(&self, r: &MediaRef) -> Result<Resolved, StorageError> {
        let p = Self::path_of(r)?;
        if !p.exists() {
            return Err(StorageError::NotFound(p.display().to_string()));
        }
        Ok(Resolved { path: Some(p), guard: Box::new(()) })
    }

    fn resolve_new(&self, r: &MediaRef) -> Result<Resolved, StorageError> {
        let p = Self::path_of(r)?;
        if let Some(dir) = p.parent() {
            std::fs::create_dir_all(dir)?;
        }
        Ok(Resolved { path: Some(p), guard: Box::new(()) })
    }

    fn open_read(&self, r: &MediaRef) -> Result<Box<dyn ReadSeek>, StorageError> {
        Ok(Box::new(File::open(Self::path_of(r)?)?))
    }

    fn open_write(&self, r: &MediaRef) -> Result<Box<dyn Write + Send>, StorageError> {
        let p = Self::path_of(r)?;
        if let Some(dir) = p.parent() {
            std::fs::create_dir_all(dir)?;
        }
        Ok(Box::new(File::create(p)?))
    }

    /// Write a temporary file beside the target, flush it to disk, then
    /// rename it over the target (atomic on one file system).
    fn write_atomic(&self, r: &MediaRef, data: &[u8]) -> Result<(), StorageError> {
        let p = Self::path_of(r)?;
        let dir = p.parent().map(Path::to_path_buf).unwrap_or_default();
        std::fs::create_dir_all(&dir)?;
        let name = p.file_name().map(|n| n.to_string_lossy().into_owned()).unwrap_or_default();
        let tmp = dir.join(format!(".{name}.{}.tmp", std::process::id()));
        let written = (|| -> std::io::Result<()> {
            let mut f = File::create(&tmp)?;
            f.write_all(data)?;
            f.sync_all()?;
            std::fs::rename(&tmp, &p)?;
            // Make the rename itself durable (best effort: not every
            // platform can open a directory).
            if let Ok(d) = File::open(&dir) {
                let _ = d.sync_all();
            }
            Ok(())
        })();
        if written.is_err() {
            let _ = std::fs::remove_file(&tmp);
        }
        Ok(written?)
    }

    fn remove(&self, r: &MediaRef) -> Result<(), StorageError> {
        match std::fs::remove_file(Self::path_of(r)?) {
            Err(e) if e.kind() != std::io::ErrorKind::NotFound => Err(e.into()),
            _ => Ok(()),
        }
    }

    fn location(&self, loc: Location, name: &str) -> Result<MediaRef, StorageError> {
        let dir = match loc {
            Location::Cache => &self.cache,
            Location::AppData => &self.app_data,
            Location::Documents => &self.documents,
        };
        Ok(MediaRef(format!("{SCHEME}{}", dir.join(name).display())))
    }

    fn display_name(&self, r: &MediaRef) -> String {
        Self::path_of(r)
            .ok()
            .and_then(|p| p.file_name().map(|n| n.to_string_lossy().into_owned()))
            .unwrap_or_else(|| r.0.clone())
    }
}

/// A system with no OS services: no processes, no dynamic code.
pub struct HeadlessSystem {
    start: Instant,
}

impl Default for HeadlessSystem {
    fn default() -> Self {
        HeadlessSystem { start: Instant::now() }
    }
}

impl System for HeadlessSystem {
    fn capabilities(&self) -> Capabilities {
        Capabilities {
            platform_name: "headless",
            processes: false,
            dynamic_libraries: false,
            jit: false,
            memory_budget: 1 << 30,
            hw_decode: vec![],
            hw_encode: vec![],
            suspends_in_background: false,
        }
    }

    fn monotonic(&self) -> Duration {
        self.start.elapsed()
    }
}

/// An audio device that plays into nothing. Tests advance it by hand with
/// [`NullAudioStream::advance`]; it never calls the render callback on its own.
#[derive(Default)]
pub struct NullAudio {
    pub clock: Arc<AudioClock>,
}

pub struct NullAudioStream {
    config: AudioConfig,
    clock: Arc<AudioClock>,
    render: Mutex<RenderCallback>,
}

impl NullAudioStream {
    /// Pretend the device played `n` frames: pulls them through the callback.
    pub fn advance(&self, n: usize) {
        let mut buf = vec![0.0; n * self.config.channels as usize];
        (self.render.lock().unwrap())(&mut buf);
        self.clock.advance(n as u64);
    }
}

impl AudioOutput for NullAudio {
    fn open(&self, want: AudioConfig, render: RenderCallback) -> Result<Box<dyn AudioStream>, AudioError> {
        Ok(Box::new(NullAudioStream { config: want, clock: self.clock.clone(), render: Mutex::new(render) }))
    }
}

impl AudioStream for NullAudioStream {
    fn play(&mut self) -> Result<(), AudioError> {
        Ok(())
    }
    fn pause(&mut self) -> Result<(), AudioError> {
        Ok(())
    }
    fn clock(&self) -> Arc<AudioClock> {
        self.clock.clone()
    }
    fn latency_frames(&self) -> u32 {
        0
    }
    fn config(&self) -> AudioConfig {
        self.config
    }
}

/// Only libraries handed to it at construction; never loads files.
#[derive(Default)]
pub struct LinkedOnly {
    pub make: Vec<fn() -> Box<dyn NativeLibrary>>,
}

impl NativeLibraries for LinkedOnly {
    fn linked(&self) -> Vec<Box<dyn NativeLibrary>> {
        self.make.iter().map(|f| f()).collect()
    }
    fn discover(&self, _kind: &str) -> Vec<String> {
        Vec::new()
    }
    fn load(&self, _path: &str) -> Result<Box<dyn NativeLibrary>, LibraryError> {
        Err(LibraryError::NotAllowed)
    }
}

/// A media backend that knows no formats; for tests that never touch media.
pub struct NoMedia;

impl MediaBackend for NoMedia {
    fn name(&self) -> &str {
        "none"
    }
    fn probe(&self, _m: &Resolved) -> Result<ve_model::MediaInfo, MediaError> {
        Err(MediaError::Unsupported("no media backend".into()))
    }
    fn open_video(&self, _m: &Resolved) -> Result<Box<dyn VideoDecoder>, MediaError> {
        Err(MediaError::Unsupported("no media backend".into()))
    }
    fn open_audio(&self, _m: &Resolved, _stream: usize, _sr: u32, _ch: u16) -> Result<Box<dyn AudioDecoder>, MediaError> {
        Err(MediaError::Unsupported("no media backend".into()))
    }
    fn open_encoder(&self, _o: &Resolved, _s: &EncoderSettings) -> Result<Box<dyn Encoder>, MediaError> {
        Err(MediaError::Unsupported("no media backend".into()))
    }
}

/// A complete headless platform with files under `root`, using `media` for
/// decoding (pass [`NoMedia`] when none is needed).
pub fn platform(root: impl AsRef<Path>, media: Arc<dyn MediaBackend>) -> Platform {
    Platform {
        system: Arc::new(HeadlessSystem::default()),
        storage: Arc::new(FileStorage::rooted(root)),
        media,
        audio: Arc::new(NullAudio::default()),
        libraries: Arc::new(LinkedOnly::default()),
        processes: None,
    }
}
