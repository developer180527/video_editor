//! Test doubles shared by the unit tests.

use ve_model::MediaRef;
use ve_ports::*;

/// Storage that resolves anything (to no path) and stores nothing.
pub struct AnyFile;
impl Storage for AnyFile {
    fn make_ref(&self, p: &str) -> Result<MediaRef, StorageError> {
        Ok(MediaRef(p.into()))
    }
    fn resolve(&self, _: &MediaRef) -> Result<Resolved, StorageError> {
        Ok(Resolved { path: None, guard: Box::new(()) })
    }
    fn resolve_new(&self, r: &MediaRef) -> Result<Resolved, StorageError> {
        self.resolve(r)
    }
    fn open_read(&self, r: &MediaRef) -> Result<Box<dyn ReadSeek>, StorageError> {
        Err(StorageError::NotFound(r.0.clone()))
    }
    fn open_write(&self, r: &MediaRef) -> Result<Box<dyn std::io::Write + Send>, StorageError> {
        Err(StorageError::NotFound(r.0.clone()))
    }
    fn location(&self, _: Location, name: &str) -> Result<MediaRef, StorageError> {
        Ok(MediaRef(name.into()))
    }
    fn display_name(&self, r: &MediaRef) -> String {
        r.0.clone()
    }
}
