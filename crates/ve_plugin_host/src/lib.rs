//! The plugin host.
//!
//! | API | Runs | Where | For |
//! |---|---|---|---|
//! | `ve` native ([`native`]) | in process | everywhere (linked in on iPadOS) | effects, transitions, generators; later codecs and importers |
//! | OpenFX ([`ofx`]) | in process | desktop | the existing ecosystem of video effects |
//! | CLAP ([`clap`]) | in process | desktop | audio effects |
//! | WASM add-ons ([`wasm`]) | in process, sandboxed | everywhere (interpreted on iPadOS) | third-party tools, scripts, automation |
//! | process add-ons ([`process`]) | separate process | where `Capabilities::processes` | AI models, heavy tools, other languages |
//!
//! Everything a plugin offers ends up as an [`EffectInfo`] in the
//! [`Registry`], whatever API it came through, so the engine and the UI never
//! care which.

pub mod clap;
pub mod intrinsic;
pub mod native;
pub mod ofx;
pub mod process;
pub mod wasm;

mod linked;
mod registry;

pub use linked::LinkedLibrary;
pub use registry::*;
