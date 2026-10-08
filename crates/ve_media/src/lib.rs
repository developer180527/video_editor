//! The media engine, between the decoders (a `MediaBackend` port) and the
//! things that need pictures and sound: the compositor, playback and export.
//!
//! - [`VideoPool`]: one background worker per source file, decoding ahead of
//!   the time last asked for into a budgeted frame cache. Asking never blocks
//!   (the UI shows the nearest frame it has); export can wait.
//! - [`Mixer`]: renders a sequence's audio — every audible clip, with Volume
//!   and Panner applied, mute and solo honoured — into interleaved stereo.
//! - [`Playback`]: runs a `Mixer` ahead of the audio device on its own thread
//!   and hands the samples to the real-time callback through a lock-free ring.

mod mixer;
mod playback;
mod stills;
mod video;

pub use stills::{Stills, Thumb, PEAKS_PER_SECOND};

pub use mixer::{db_to_gain, Mixer};
pub use playback::{Meters, Playback};
pub use video::{Lookup, VideoPool};

use std::sync::Arc;

/// Called when something new can be shown (a frame arrived).
pub type Waker = Arc<dyn Fn() + Send + Sync>;
