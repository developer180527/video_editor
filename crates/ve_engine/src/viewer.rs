//! The two monitors. The Program monitor shows the active sequence; the
//! Source monitor shows one asset on its own, for marking in and out before
//! it goes into the sequence.
//!
//! One transport plays whichever monitor is active ([`Viewer`]); the other's
//! position waits, stopped, until it is active again. An asset in the
//! Source monitor plays as a sequence of its own — its picture and every
//! audio stream, the way the editor would cut it in — which the mixer and
//! compositor play like any other sequence. That sequence is never part of
//! the project: frames and audio see it in a copy of the project with it
//! added and active.

use std::sync::Arc;

use ve_model::*;
use ve_plugin_host::Registry;
use ve_time::Time;

/// Which monitor the transport plays.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Hash)]
pub enum Viewer {
    #[default]
    Program,
    Source,
}

/// What the Source monitor holds.
#[derive(Clone, Debug)]
pub struct SourceView {
    pub asset: AssetId,
    /// The asset as a sequence of its own (not in the project).
    pub sequence: Arc<Sequence>,
}

impl SourceView {
    /// `asset` of `project` ready to play: `None` for media not yet probed.
    pub fn new(registry: &Registry, project: &Project, asset: AssetId) -> Option<SourceView> {
        let a = project.assets.get(&asset)?;
        let info = a.info.as_ref()?;
        let mut format = project.active().map(|s| s.format.clone()).unwrap_or_default();
        if let Some(v) = &info.video {
            (format.width, format.height) = v.display_size();
            format.rate = v.rate;
        }
        let duration = if info.duration > Time::ZERO { info.duration } else { Time::from_seconds(5) };
        let mut tracks = vec![Track::new(TrackKind::Video, "V1")];
        for k in 0..info.audio.len().max(1) {
            tracks.push(Track::new(TrackKind::Audio, format!("A{}", k + 1)));
        }
        let mut seq = Sequence::new(format!("Source: {}", a.name), format, tracks);
        let (v1, a1) = (seq.tracks[0].id, seq.tracks[1].id);
        let (_, items) = crate::clips_for_asset(registry, &seq, a, duration, Some(v1), Some(a1));
        for (track, clip) in items {
            if let Some(t) = seq.tracks.iter_mut().find(|t| t.id == track) {
                Arc::make_mut(t).clips.push_back(clip);
            }
        }
        Some(SourceView { asset, sequence: Arc::new(seq) })
    }

    /// `project` with this source's sequence added and active: what frames
    /// and audio of the Source monitor are made from.
    pub fn project(&self, project: &Project) -> Snapshot {
        let mut p = project.clone();
        p.sequences.insert(self.sequence.id, self.sequence.clone());
        p.active_sequence = Some(self.sequence.id);
        Arc::new(p)
    }

    pub fn duration(&self) -> Time {
        self.sequence.duration()
    }
}
