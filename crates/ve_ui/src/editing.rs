//! Lift, Extract and Match Frame: taking a range out of the sequence, and
//! finding a sequence frame's source (and back).

use libgui::*;
use ve_engine::{Command, MarksOwner, edit};
use ve_model::*;
use ve_time::{Time, TimeRange};

use crate::EditorUi;

/// The frame of `clip` (its own time from its start) that shows source
/// time `media`, if it shows it at all. Exact for normal speed; otherwise
/// the first frame that lands on it (speed changes and remaps included).
pub(crate) fn clip_time_of(clip: &Clip, media: Time, rate: ve_time::Rate) -> Option<Time> {
    if !clip.media_extent().contains(media) {
        return None;
    }
    if clip.retime.is_normal() {
        return Some(media - clip.source_range.start);
    }
    let target = media.to_frame(rate);
    let frames = clip.source_range.duration.to_frame(rate).max(1);
    (0..frames).map(|k| rate.frame_to_time(k)).find(|t| clip.media_time(*t).to_frame(rate) == target)
}

impl EditorUi {
    /// The sequence's in to out (its start or end where one is unset);
    /// `None` when neither is set: lifting everything is never what a
    /// stray key press meant.
    fn marked_range(&mut self) -> Option<(Sequence, TimeRange)> {
        let seq = self.active_seq()?;
        let (i, o) = (seq.marks.in_point, seq.marks.out_point);
        if i.is_none() && o.is_none() {
            self.errors.push("Mark an In and Out on the sequence first (I and O).".into());
            return None;
        }
        let (start, end) = (i.unwrap_or(Time::ZERO), o.unwrap_or(seq.duration()));
        if end <= start {
            self.errors.push("The sequence's Out is not after its In.".into());
            return None;
        }
        Some(((*seq).clone(), TimeRange::new(start, end - start)))
    }

    /// Lift (;): take the sequence's in-to-out out of the targeted tracks,
    /// leaving a gap. The marks stay, ready for the next edit.
    pub(crate) fn lift(&mut self) {
        let Some((seq, range)) = self.marked_range() else {
            return;
        };
        let tracks: Vec<TrackId> = seq.tracks.iter().filter(|t| self.view.targeted.contains(&t.id) && !t.locked).map(|t| t.id).collect();
        if tracks.is_empty() {
            self.errors.push("Target the tracks to lift from.".into());
            return;
        }
        let r = edit::lift_range(self.snap(), seq.id, &tracks, range);
        self.run_edit(r);
    }

    /// Extract ('): take the sequence's in-to-out out of every unlocked
    /// track and close the gap, so nothing slips out of sync. The playhead
    /// parks at the join; the marks go (the range no longer exists).
    pub(crate) fn extract(&mut self) {
        let Some((seq, range)) = self.marked_range() else {
            return;
        };
        let tracks: Vec<TrackId> = seq.tracks.iter().filter(|t| !t.locked).map(|t| t.id).collect();
        let r = edit::extract_range(self.snap(), seq.id, &tracks, range).map(|cmd| {
            let marks = Marks { in_point: None, out_point: None, ..seq.marks.clone() };
            // One undo step: the edit and the cleared marks.
            Command::Batch { label: "Extract".into(), commands: vec![cmd, Command::SetMarks { owner: MarksOwner::Sequence(seq.id), marks }] }
        });
        if r.is_ok() {
            self.seek(range.start);
        }
        self.run_edit(r);
    }

    /// The clip Match Frame matches: on the targeted video tracks from the
    /// top, then any video track, then the audio.
    fn clip_for_match(&self) -> Option<Clip> {
        let seq = self.active_seq()?;
        let t = self.playhead;
        let video = seq.tracks.iter().rev().filter(|tr| tr.kind == TrackKind::Video && tr.enabled);
        let targeted = video.clone().filter(|tr| self.view.targeted.contains(&tr.id));
        let audio = seq.tracks.iter().filter(|tr| tr.kind == TrackKind::Audio);
        targeted.chain(video).chain(audio).find_map(|tr| tr.clip_at(t).filter(|c| c.enabled).map(|c| (**c).clone()))
    }

    /// Match Frame (F): open the clip under the playhead in the Source
    /// monitor at the very frame showing, marked to the part the clip uses.
    /// A nested sequence opens instead, at that frame.
    pub(crate) fn match_frame(&mut self) {
        let Some(clip) = self.clip_for_match() else {
            self.errors.push("No clip under the playhead to match.".into());
            return;
        };
        let media = clip.source_time(self.playhead);
        match clip.source {
            ClipSource::Asset { asset, .. } => {
                let extent = clip.media_extent();
                let marks = self.snap().assets.get(&asset).map(|a| a.marks.clone()).unwrap_or_default();
                let marks = Marks { in_point: Some(extent.start), out_point: Some(extent.end()), ..marks };
                self.run(Command::SetMarks { owner: MarksOwner::Asset(asset), marks });
                self.open_in_source(asset);
                // The Source monitor opens at the in point; then this frame.
                self.source_seek_on_open = Some((asset, media));
            }
            ClipSource::Sequence { sequence, .. } => {
                self.open_sequence(sequence);
                self.seek(media);
            }
            ClipSource::Generator { .. } => self.errors.push("A generated clip has no source to match.".into()),
        }
    }

    /// Reverse Match Frame (Shift+R): put the program playhead on the
    /// sequence frame that shows the Source monitor's frame.
    pub(crate) fn reverse_match_frame(&mut self) {
        let Some(source) = self.engine.source() else {
            self.errors.push("Open a clip in the Source monitor first.".into());
            return;
        };
        let Some(seq) = self.active_seq() else { return };
        let media = self.source_playhead;
        let rate = seq.format.rate;
        // The first use in the sequence, from the top track down.
        let found = seq.tracks.iter().rev().flat_map(|t| t.clips.iter()).find_map(|c| match c.source {
            ClipSource::Asset { asset, .. } if asset == source.asset => clip_time_of(c, media, rate).map(|t| c.timeline_start + t),
            _ => None,
        });
        match found {
            Some(t) => {
                self.engine.stop();
                self.seek(t);
            }
            None => self.errors.push("That frame is not used in the sequence.".into()),
        }
    }
}

/// Lift, Extract and Match Frame shortcuts.
pub(crate) fn shortcuts(app: &mut EditorUi, ui: &mut Ui) {
    let key = |ui: &mut Ui, k: Key| ui.consume_shortcut(Shortcut::plain(k));
    if key(ui, Key::Semicolon) {
        app.lift();
    }
    if key(ui, Key::Quote) {
        app.extract();
    }
    if key(ui, Key::F) {
        app.match_frame();
    }
    if ui.consume_shortcut(Shortcut::plain(Key::R).shift()) {
        app.reverse_match_frame();
    }
}
