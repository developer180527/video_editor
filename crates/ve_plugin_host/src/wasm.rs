//! Sandboxed WebAssembly add-ons (all platforms).
//!
//! An add-on is a WASM component speaking the add-on protocol (the same
//! messages as process add-ons: commands in, events and snapshots out). It
//! runs in-process but cannot touch memory outside its sandbox, so a crash
//! traps instead of taking the app down. JIT-compiled where
//! `Capabilities::jit` is true, interpreted elsewhere (iPadOS).
//!
//! Not implemented in Phase A. The runtime (wasmtime / wasmi) is chosen in
//! Phase D; the protocol is shared with [`crate::process`].
