//! The document.
//!
//! A [`Project`] is a value. Cloning it is cheap (the collections are
//! persistent, structurally shared), so every edit produces a new `Project`
//! and the old one stays valid for whoever still reads it: the UI drawing
//! this frame, the playback thread rendering the next, an export running in
//! the background. Nobody locks the document.
//!
//! Rules this crate keeps:
//! - Times are [`ve_time::Time`] ticks; no floats for time.
//! - Every object has a stable [`Id`]; references between objects are ids,
//!   never indices or pointers.
//! - Media is referenced by a [`MediaRef`] (an opaque storage handle), never by
//!   a filesystem path — on iPad there is no path to keep.
//! - Only `ve_command` changes a project. Everything else reads.

mod ids;
mod param;
mod validate;

pub use ids::*;
pub use param::*;
pub use validate::{validate, ModelError};

pub use imbl::{OrdMap, Vector};
use serde::{Deserialize, Serialize};
use std::sync::Arc;
use ve_time::{Rate, Time, TimeRange};

/// Bumped whenever the serialized shape changes; loaders migrate older files.
pub const SCHEMA_VERSION: u32 = 1;

/// A read-only view of the document, shared across threads.
pub type Snapshot = Arc<Project>;

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct Project {
    pub schema_version: u32,
    pub name: String,
    pub assets: OrdMap<AssetId, Arc<Asset>>,
    pub sequences: OrdMap<SequenceId, Arc<Sequence>>,
    /// The sequence the editor opens on.
    pub active_sequence: Option<SequenceId>,
}

impl Project {
    pub fn new(name: impl Into<String>) -> Self {
        Project {
            schema_version: SCHEMA_VERSION,
            name: name.into(),
            assets: OrdMap::new(),
            sequences: OrdMap::new(),
            active_sequence: None,
        }
    }

    pub fn sequence(&self, id: SequenceId) -> Option<&Arc<Sequence>> {
        self.sequences.get(&id)
    }

    pub fn active(&self) -> Option<&Arc<Sequence>> {
        self.active_sequence.and_then(|id| self.sequences.get(&id))
    }

    /// The clip with `id`, and the sequence and track holding it.
    pub fn find_clip(&self, id: ClipId) -> Option<(&Arc<Sequence>, usize, &Arc<Clip>)> {
        self.sequences.values().find_map(|s| {
            s.tracks.iter().enumerate().find_map(|(ti, t)| t.clips.iter().find(|c| c.id == id).map(|c| (s, ti, c)))
        })
    }
}

/// Where media lives, as the platform's `Storage` port understands it: a
/// path on desktop, a security-scoped bookmark on iPad, a URL for a remote
/// asset. The model never interprets it.
#[derive(Clone, Debug, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub struct MediaRef(pub String);

/// An imported file in the project bin.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct Asset {
    pub id: AssetId,
    pub name: String,
    pub media: MediaRef,
    /// Filled once the file has been probed.
    pub info: Option<MediaInfo>,
}

/// What a probe found in a media file.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct MediaInfo {
    pub duration: Time,
    pub video: Option<VideoStreamInfo>,
    pub audio: Option<AudioStreamInfo>,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct VideoStreamInfo {
    pub width: u32,
    pub height: u32,
    pub rate: Rate,
    pub codec: String,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct AudioStreamInfo {
    pub sample_rate: u32,
    pub channels: u16,
    pub codec: String,
}

/// The format a sequence renders at.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct SequenceFormat {
    pub width: u32,
    pub height: u32,
    pub rate: Rate,
    pub sample_rate: u32,
    /// An OpenColorIO colour space name, e.g. "Rec.1886 Rec.709 - Display".
    pub working_space: String,
}

impl Default for SequenceFormat {
    fn default() -> Self {
        SequenceFormat {
            width: 1920,
            height: 1080,
            rate: Rate::FPS_24,
            sample_rate: 48_000,
            working_space: "ACEScg".into(),
        }
    }
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct Sequence {
    pub id: SequenceId,
    pub name: String,
    pub format: SequenceFormat,
    /// Video tracks first, bottom (V1) to top; then audio tracks, A1 down.
    pub tracks: Vector<Arc<Track>>,
}

impl Sequence {
    pub fn track(&self, id: TrackId) -> Option<(usize, &Arc<Track>)> {
        self.tracks.iter().enumerate().find(|(_, t)| t.id == id)
    }

    /// The end of the last clip.
    pub fn duration(&self) -> Time {
        self.tracks.iter().filter_map(|t| t.clips.last().map(|c| c.timeline_range().end())).max().unwrap_or(Time::ZERO)
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub enum TrackKind {
    Video,
    Audio,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct Track {
    pub id: TrackId,
    pub kind: TrackKind,
    pub name: String,
    /// Sorted by `timeline_start`; never overlapping (see [`validate`]).
    pub clips: Vector<Arc<Clip>>,
    pub enabled: bool,
    pub locked: bool,
    pub muted: bool,
    pub solo: bool,
}

impl Track {
    pub fn new(kind: TrackKind, name: impl Into<String>) -> Self {
        Track {
            id: TrackId::new(),
            kind,
            name: name.into(),
            clips: Vector::new(),
            enabled: true,
            locked: false,
            muted: false,
            solo: false,
        }
    }

    /// The clip under `t`, if any.
    pub fn clip_at(&self, t: Time) -> Option<&Arc<Clip>> {
        let i = partition_point(&self.clips, |c| c.timeline_start <= t);
        i.checked_sub(1).map(|i| &self.clips[i]).filter(|c| c.timeline_range().contains(t))
    }

    /// Whether `range` is free of clips, ignoring `except`.
    pub fn is_free(&self, range: TimeRange, except: Option<ClipId>) -> bool {
        self.clips.iter().all(|c| Some(c.id) == except || !c.timeline_range().overlaps(range))
    }

    /// Index at which a clip starting at `start` keeps the track sorted.
    pub fn insertion_index(&self, start: Time) -> usize {
        partition_point(&self.clips, |c| c.timeline_start <= start)
    }
}

/// First index where `pred` is false, for a vector partitioned by `pred`.
pub fn partition_point<T: Clone>(v: &Vector<T>, pred: impl Fn(&T) -> bool) -> usize {
    use std::cmp::Ordering;
    v.binary_search_by(|x| if pred(x) { Ordering::Less } else { Ordering::Greater }).unwrap_or_else(|i| i)
}

/// What a clip plays.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub enum ClipSource {
    /// A span of an imported file.
    Asset { asset: AssetId },
    /// Synthesised by a plugin: a title, a colour matte, bars and tone.
    Generator { plugin: PluginRef },
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct Clip {
    pub id: ClipId,
    pub name: String,
    pub source: ClipSource,
    /// The span of the source that plays, in source time.
    pub source_range: TimeRange,
    /// Where the clip starts on the sequence.
    pub timeline_start: Time,
    pub enabled: bool,
    /// Clips that move and trim together (a video clip and its audio).
    pub link: Option<LinkId>,
    /// Applied bottom to top: `effects[0]` sees the source first.
    pub effects: Vector<Arc<Effect>>,
}

impl Clip {
    pub fn timeline_range(&self) -> TimeRange {
        TimeRange::new(self.timeline_start, self.source_range.duration)
    }

    /// The source time shown at sequence time `t`.
    pub fn source_time(&self, t: Time) -> Time {
        self.source_range.start + (t - self.timeline_start)
    }
}

/// Which plugin implements something, and through which API.
#[derive(Clone, Debug, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub struct PluginRef {
    pub api: PluginApi,
    /// Reverse-DNS identifier, e.g. "com.example.blur".
    pub id: String,
    pub major_version: u32,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub enum PluginApi {
    /// Built into the app (still goes through the `ve` ABI).
    Builtin,
    /// Native `ve_plugin.h` plugin.
    Native,
    /// OpenFX image effect.
    OpenFx,
    /// CLAP audio plugin.
    Clap,
    /// Sandboxed WebAssembly add-on.
    Wasm,
}

/// One effect instance on a clip.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct Effect {
    pub id: EffectId,
    pub plugin: PluginRef,
    pub enabled: bool,
    /// Keyed by the plugin's parameter id.
    pub params: OrdMap<String, Param>,
}

#[cfg(test)]
mod tests;
