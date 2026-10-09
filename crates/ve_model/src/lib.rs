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
mod timing;
mod validate;

pub use ids::*;
pub use param::*;
pub use timing::*;
pub use validate::{validate, ModelError};

pub use imbl::{OrdMap, Vector};
use serde::{Deserialize, Serialize};
use std::sync::Arc;
use ve_time::{Rate, Time, TimeRange};

/// Bumped whenever the serialized shape changes; loaders migrate older files.
/// 2: `MediaInfo::audio` lists every audio stream; clips name theirs.
/// 3: speed and remapping, transitions, nested sequences and multicam,
///    media variants, markers and in/out points, track channel layouts.
///    (All new fields have defaults: a schema-2 file reads as is.)
pub const SCHEMA_VERSION: u32 = 3;

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
    /// The original. Relinking replaces it (`Command::SetAssetMedia`).
    pub media: MediaRef,
    /// Filled once the file has been probed.
    pub info: Option<MediaInfo>,
    /// Other renditions of the same media: proxies for smooth editing.
    #[serde(default)]
    pub variants: Vec<MediaVariant>,
    /// Source-monitor in/out points and clip markers, in source time.
    #[serde(default)]
    pub marks: Marks,
}

impl Asset {
    /// A new asset with no variants or marks.
    pub fn new(name: impl Into<String>, media: MediaRef, info: Option<MediaInfo>) -> Self {
        Asset { id: AssetId::new(), name: name.into(), media, info, variants: Vec::new(), marks: Marks::default() }
    }

    /// The rendition to read pictures from: a proxy when `proxies` and one
    /// exists, else the original. Pictures keep the original's logical size
    /// (see [`MediaVariant`]); sound always comes from the original.
    pub fn picture_media(&self, proxies: bool) -> &MediaRef {
        match self.variants.iter().find(|v| v.kind == VariantKind::Proxy) {
            Some(v) if proxies => &v.media,
            _ => &self.media,
        }
    }
}

/// Another rendition of an asset's media.
///
/// A proxy is smaller, but everything about a clip — Motion positions,
/// anchors, crops — stays in the original's pixels; a proxy frame is
/// stretched to the original's size before any of it applies.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct MediaVariant {
    pub kind: VariantKind,
    pub media: MediaRef,
    pub width: u32,
    pub height: u32,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub enum VariantKind {
    /// For editing: lighter to decode. Never used for export.
    Proxy,
}

/// In/out points and markers, on a sequence (sequence time) or an asset
/// (source time).
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
pub struct Marks {
    pub in_point: Option<Time>,
    pub out_point: Option<Time>,
    /// Sorted by time.
    pub markers: Vector<Marker>,
}

impl Marks {
    /// The span between the in and out points, defaulting to `0..end`.
    pub fn range(&self, end: Time) -> TimeRange {
        let a = self.in_point.unwrap_or(Time::ZERO);
        let b = self.out_point.unwrap_or(end).max(a);
        TimeRange::from_bounds(a, b)
    }
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct Marker {
    pub id: MarkerId,
    pub time: Time,
    /// Zero for a point marker; a span otherwise.
    pub duration: Time,
    pub name: String,
    pub comment: String,
    pub color: MarkerColor,
    pub kind: MarkerKind,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub enum MarkerColor {
    Green,
    Red,
    Purple,
    Orange,
    Yellow,
    White,
    Blue,
    Cyan,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub enum MarkerKind {
    Comment,
    /// A chapter point, written to exported files that carry chapters.
    Chapter,
}

/// What a probe found in a media file.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct MediaInfo {
    pub duration: Time,
    pub video: Option<VideoStreamInfo>,
    /// Every audio stream, in file order. Cameras often record each channel
    /// as a stream of its own (eight mono streams, say); each can be a clip.
    pub audio: Vec<AudioStreamInfo>,
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
    /// The channel layout as FFmpeg names it: "mono", "stereo", "5.1",
    /// "7.1"; or "N channels" when the file does not say.
    #[serde(default)]
    pub layout: String,
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
    /// In/out points (what export renders) and sequence markers.
    #[serde(default)]
    pub marks: Marks,
}

impl Sequence {
    pub fn track(&self, id: TrackId) -> Option<(usize, &Arc<Track>)> {
        self.tracks.iter().enumerate().find(|(_, t)| t.id == id)
    }

    /// A new sequence with `tracks` and no marks.
    pub fn new(name: impl Into<String>, format: SequenceFormat, tracks: impl IntoIterator<Item = Track>) -> Self {
        Sequence { id: SequenceId::new(), name: name.into(), format, tracks: tracks.into_iter().map(Arc::new).collect(), marks: Marks::default() }
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
    /// An audio track's channels: what its clips are mixed to before the
    /// track's pan places it in the stereo master.
    #[serde(default)]
    pub layout: ChannelLayout,
}

/// Channels of an audio track.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub enum ChannelLayout {
    /// One channel; panned into the master.
    Mono,
    #[default]
    Stereo,
}

impl ChannelLayout {
    pub fn channels(self) -> u16 {
        match self {
            ChannelLayout::Mono => 1,
            ChannelLayout::Stereo => 2,
        }
    }
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
            layout: ChannelLayout::default(),
        }
    }

    /// The clip under `t`, if any.
    pub fn clip_at(&self, t: Time) -> Option<&Arc<Clip>> {
        let i = partition_point(&self.clips, |c| c.timeline_start <= t);
        i.checked_sub(1).map(|i| &self.clips[i]).filter(|c| c.timeline_range().contains(t))
    }

    /// Whether `range` is free of clips, ignoring `except`. O(log n): the
    /// clips are sorted and do not overlap, so only the neighbours of where
    /// `range` would go can touch it.
    pub fn is_free(&self, range: TimeRange, except: Option<ClipId>) -> bool {
        let i = self.insertion_index(range.start);
        // Up to two on each side, in case one of them is `except`.
        (i.saturating_sub(2)..(i + 2).min(self.clips.len()))
            .map(|j| &self.clips[j])
            .all(|c| Some(c.id) == except || !c.timeline_range().overlaps(range))
    }

    /// Index at which a clip starting at `start` keeps the track sorted.
    pub fn insertion_index(&self, start: Time) -> usize {
        partition_point(&self.clips, |c| c.timeline_start <= start)
    }

    /// Whether clip `i` runs straight into clip `i + 1` (a cut).
    fn adjacent(&self, i: usize) -> bool {
        self.clips.get(i + 1).is_some_and(|n| n.timeline_start == self.clips[i].timeline_range().end())
    }

    /// The transition at clip `i`'s head: the cut from the clip before (if
    /// adjacent) or a fade from nothing. `(region, outgoing clip index)`.
    fn head(&self, i: usize) -> Option<(TimeRange, Option<usize>, &Arc<Transition>)> {
        let c = &self.clips[i];
        let tr = c.transition_in.as_ref()?;
        let region = TimeRange::new(c.timeline_start - tr.before, tr.duration());
        let from = i.checked_sub(1).filter(|&p| self.adjacent(p));
        Some((region, from, tr))
    }

    /// The fade to nothing at clip `i`'s tail, unless a clip follows
    /// directly (then that clip's head owns the cut).
    fn tail(&self, i: usize) -> Option<(TimeRange, &Arc<Transition>)> {
        let c = &self.clips[i];
        let tr = c.transition_out.as_ref().filter(|_| !self.adjacent(i))?;
        Some((TimeRange::new(c.timeline_range().end() - tr.before, tr.duration()), tr))
    }

    /// What plays on this track at `t`: the clip there, or — inside a
    /// transition — the clips going out and coming in, with how far along
    /// the transition is. At most two clips, given [`validate`]'s rule that
    /// transition regions on a track never overlap.
    pub fn active_at(&self, t: Time) -> Vec<Active<'_>> {
        let next = self.insertion_index(t);
        // Only the clip under (or just before) `t` and the one after can
        // have a transition reaching `t`.
        for i in [next.checked_sub(1), Some(next)].into_iter().flatten().filter(|&i| i < self.clips.len()) {
            if let Some((r, from, tr)) = self.head(i).filter(|(r, ..)| r.contains(t)) {
                let progress = (t - r.start).ticks() as f64 / r.duration.ticks() as f64;
                let mut v = Vec::with_capacity(2);
                if let Some(o) = from {
                    v.push(Active { clip: &self.clips[o], transition: Some((tr, progress, Role::Outgoing)) });
                }
                v.push(Active { clip: &self.clips[i], transition: Some((tr, progress, Role::Incoming)) });
                return v;
            }
            if let Some((r, tr)) = self.tail(i).filter(|(r, _)| r.contains(t)) {
                let progress = (t - r.start).ticks() as f64 / r.duration.ticks() as f64;
                return vec![Active { clip: &self.clips[i], transition: Some((tr, progress, Role::Outgoing)) }];
            }
        }
        self.clip_at(t).map(|c| vec![Active { clip: c, transition: None }]).unwrap_or_default()
    }

    /// How far clip `i` reaches on the timeline, transitions included: it
    /// plays into its media handles before its start and after its end.
    pub fn reach(&self, i: usize) -> TimeRange {
        let c = &self.clips[i];
        let range = c.timeline_range();
        let start = self.head(i).map_or(range.start, |(r, ..)| r.start.min(range.start));
        let mut end = self.tail(i).map_or(range.end(), |(r, _)| r.end().max(range.end()));
        if let Some((r, Some(_), _)) = self.clips.get(i + 1).and_then(|_| self.head(i + 1)) {
            end = end.max(r.end());
        }
        TimeRange::from_bounds(start, end)
    }
}

/// A clip playing at some moment, and its part in a transition then.
#[derive(Clone, Copy, Debug)]
pub struct Active<'a> {
    pub clip: &'a Arc<Clip>,
    /// The transition, how far through it (0..1), and this clip's role.
    pub transition: Option<(&'a Arc<Transition>, f64, Role)>,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Role {
    Outgoing,
    Incoming,
}

/// First index where `pred` is false, for a vector partitioned by `pred`.
pub fn partition_point<T: Clone>(v: &Vector<T>, pred: impl Fn(&T) -> bool) -> usize {
    use std::cmp::Ordering;
    v.binary_search_by(|x| if pred(x) { Ordering::Less } else { Ordering::Greater }).unwrap_or_else(|i| i)
}

/// What a clip plays.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub enum ClipSource {
    /// A span of an imported file. An audio clip plays one of its audio
    /// streams (an index into `MediaInfo::audio`), mixed to the sequence by
    /// that stream's channel layout.
    Asset {
        asset: AssetId,
        #[serde(default)]
        audio_stream: u32,
    },
    /// Synthesised by a plugin: a title, a colour matte, bars and tone. Its
    /// parameters live in the clip's effect with the same `plugin`, so they
    /// animate, reset and undo like any effect's.
    Generator { plugin: PluginRef },
    /// Another sequence of the project, as one clip: a compound clip, or —
    /// with `angle` — a multicam clip showing one angle (video track
    /// `angle` of the nested sequence) while its sound plays the whole mix.
    Sequence {
        sequence: SequenceId,
        #[serde(default)]
        angle: Option<u32>,
    },
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
    /// Speed or time remapping (see [`Retime`]). The clip's length on the
    /// timeline is `source_range.duration` either way.
    #[serde(default)]
    pub retime: Retime,
    /// The transition into this clip: from the clip ending where this one
    /// starts on the same track, or — when there is none — in from nothing
    /// (a fade). Owning it here means no edit can leave it dangling.
    #[serde(default)]
    pub transition_in: Option<Arc<Transition>>,
    /// Out to nothing at the clip's end (a fade). Ignored where another
    /// clip follows directly: that clip's `transition_in` owns the cut.
    #[serde(default)]
    pub transition_out: Option<Arc<Transition>>,
    /// Audio clips: which channels of the source stream feed the clip, in
    /// order; empty for all of them, mixed to the track's layout.
    #[serde(default)]
    pub channels: Vec<u16>,
}

impl Clip {
    pub fn timeline_range(&self) -> TimeRange {
        TimeRange::new(self.timeline_start, self.source_range.duration)
    }

    /// The media time shown `t` into the clip (clip time; may lie outside
    /// `0..duration` inside a transition, reaching into the handles).
    pub fn media_time(&self, t: Time) -> Time {
        self.source_range.start + self.retime.offset(t)
    }

    /// The source time shown at sequence time `t`.
    pub fn source_time(&self, t: Time) -> Time {
        self.media_time(t - self.timeline_start)
    }

    /// The span of media the clip plays (lowest to highest media time).
    pub fn media_extent(&self) -> TimeRange {
        let (lo, hi) = self.retime.extent(self.source_range.duration);
        TimeRange::from_bounds(self.source_range.start + lo, self.source_range.start + hi)
    }

    /// The clip with its start moved `d` later in its own time — a head
    /// trim by `d`, or the right part of a split `d` in — keeping every
    /// frame (and remap key) on the media it showed.
    pub fn with_start_advanced(&self, d: Time) -> Clip {
        let mut c = self.clone();
        let (offset, retime) = self.retime.advanced(d);
        c.timeline_start += d;
        c.source_range.start += offset;
        c.source_range.duration -= d;
        c.retime = retime;
        c
    }
}

/// A transition at a clip's edge: a cross-dissolve with the neighbouring
/// clip, or a fade from or to nothing.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct Transition {
    pub id: EffectId,
    /// e.g. `ve.dissolve` (video), `ve.crossfade` (audio).
    pub plugin: PluginRef,
    /// How far it reaches before and after the edge. Centred, start-at-cut
    /// and end-at-cut alignments are just different splits.
    pub before: Time,
    pub after: Time,
    /// Keyed by the plugin's parameter id; keyframes relative to the
    /// transition's start.
    pub params: OrdMap<String, Param>,
}

impl Transition {
    pub fn duration(&self) -> Time {
        self.before + self.after
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
