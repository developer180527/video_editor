use std::ffi::c_void;
use thiserror::Error;

#[derive(Debug, Error)]
pub enum LibraryError {
    #[error("not found: {0}")]
    NotFound(String),
    #[error("missing symbol {0}")]
    MissingSymbol(String),
    #[error("loading native code from files is not allowed on this platform")]
    NotAllowed,
    #[error("{0}")]
    Other(String),
}

/// A loaded native plugin binary — or, where `dlopen` is forbidden, a table
/// of entry points linked into the app. The plugin host cannot tell which.
pub trait NativeLibrary: Send + Sync {
    fn name(&self) -> &str;
    /// The address of an exported C function.
    fn symbol(&self, name: &str) -> Result<*const c_void, LibraryError>;
}

pub trait NativeLibraries: Send + Sync {
    /// Libraries that exist without searching: built-ins, and everything
    /// statically linked on platforms that cannot load files.
    fn linked(&self) -> Vec<Box<dyn NativeLibrary>>;
    /// Plugin files found in the platform's plugin folders, by kind
    /// ("ve", "ofx", "clap").
    fn discover(&self, kind: &str) -> Vec<String>;
    /// Load one file found by `discover`.
    fn load(&self, path: &str) -> Result<Box<dyn NativeLibrary>, LibraryError>;
}
