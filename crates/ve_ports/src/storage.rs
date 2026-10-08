use std::any::Any;
use std::io::{Read, Seek, Write};
use std::path::PathBuf;
use thiserror::Error;
use ve_model::MediaRef;

#[derive(Debug, Error)]
pub enum StorageError {
    #[error("not found: {0}")]
    NotFound(String),
    #[error("permission denied: {0}")]
    Denied(String),
    #[error("the reference is stale; ask the user to locate the file again")]
    Stale,
    #[error(transparent)]
    Io(#[from] std::io::Error),
}

pub trait ReadSeek: Read + Seek + Send {}
impl<T: Read + Seek + Send> ReadSeek for T {}

/// A media reference made usable. While it is alive the file may be read
/// (on iPadOS it holds the security scope open); drop it when done.
pub struct Resolved {
    /// A real path when one exists — the fast path for FFmpeg. `None` for
    /// media only reachable as a stream.
    pub path: Option<PathBuf>,
    pub guard: Box<dyn Any + Send>,
}

/// Where well-known app data lives.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Location {
    /// Rebuildable: proxies, thumbnails, peak files. The OS may purge it.
    Cache,
    /// Autosaves, settings, layouts.
    AppData,
    /// Where the user's projects go by default.
    Documents,
}

/// Files, without paths. The model stores [`MediaRef`]s; only the storage
/// adapter knows whether one is a path, a bookmark or a URL.
pub trait Storage: Send + Sync {
    /// Turn something the user picked (a path, a document-picker URL) into a
    /// reference that survives restarts.
    fn make_ref(&self, picked: &str) -> Result<MediaRef, StorageError>;
    fn resolve(&self, r: &MediaRef) -> Result<Resolved, StorageError>;
    /// Make `r` writable as a new file (an export target): its folder exists,
    /// and on sandboxed platforms access is held while `Resolved` lives.
    fn resolve_new(&self, r: &MediaRef) -> Result<Resolved, StorageError>;
    fn open_read(&self, r: &MediaRef) -> Result<Box<dyn ReadSeek>, StorageError>;
    fn open_write(&self, r: &MediaRef) -> Result<Box<dyn Write + Send>, StorageError>;
    /// Replace `r` with `data` all at once: afterwards the file holds either
    /// the old contents or the new, never a mix or a truncated file, even if
    /// the app dies or the disk fills mid-write. For documents.
    ///
    /// The default is not atomic; adapters with real files override it.
    fn write_atomic(&self, r: &MediaRef, data: &[u8]) -> Result<(), StorageError> {
        let mut w = self.open_write(r)?;
        w.write_all(data)?;
        w.flush()?;
        Ok(())
    }
    /// Delete `r` if it exists.
    fn remove(&self, _r: &MediaRef) -> Result<(), StorageError> {
        Ok(())
    }
    /// A reference to `name` inside a well-known location.
    fn location(&self, loc: Location, name: &str) -> Result<MediaRef, StorageError>;
    /// What to show the user.
    fn display_name(&self, r: &MediaRef) -> String;
}
