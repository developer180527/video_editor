use std::ffi::c_void;
use ve_ports::{LibraryError, NativeLibrary};

/// A "library" whose functions are linked into the app: how built-ins work
/// everywhere and how every native plugin works on iPadOS.
pub struct LinkedLibrary {
    pub name: String,
    pub symbols: Vec<(String, usize)>,
}

impl LinkedLibrary {
    pub fn new(name: impl Into<String>) -> Self {
        LinkedLibrary { name: name.into(), symbols: Vec::new() }
    }

    pub fn with(mut self, symbol: impl Into<String>, address: *const c_void) -> Self {
        self.symbols.push((symbol.into(), address as usize));
        self
    }
}

impl NativeLibrary for LinkedLibrary {
    fn name(&self) -> &str {
        &self.name
    }

    fn symbol(&self, name: &str) -> Result<*const c_void, LibraryError> {
        self.symbols
            .iter()
            .find(|(n, _)| n == name)
            .map(|(_, a)| *a as *const c_void)
            .ok_or_else(|| LibraryError::MissingSymbol(name.into()))
    }
}
