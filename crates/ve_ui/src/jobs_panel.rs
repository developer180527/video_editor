//! The Jobs panel: exports and proxies, running, queued and finished, with
//! what can be done to each (pause, resume, cancel, retry, remove).
//!
//! The list is the engine's job queue (`ve_engine::Jobs`); this panel only
//! shows it and passes the buttons on. While anything is active the UI
//! looks again four times a second — enough for a progress bar, and the
//! editor sleeps otherwise.

use libgui::*;
use ve_engine::{JobInfo, JobState};

use crate::theme::REEL;
use crate::EditorUi;

/// The tab's title: with the number of jobs still to do, when there are any.
pub(crate) fn title(app: &EditorUi) -> String {
    let open = app.engine.jobs().list().iter().filter(|j| !j.state.finished()).count();
    if open == 0 { "Jobs".into() } else { format!("Jobs ({open})") }
}

fn duration(d: std::time::Duration) -> String {
    let s = d.as_secs();
    if s >= 3600 { format!("{}:{:02}:{:02}", s / 3600, s / 60 % 60, s % 60) } else { format!("{}:{:02}", s / 60, s % 60) }
}

/// Running first, then queued (and paused, held), then finished, newest first.
fn order(j: &JobInfo) -> (u8, i64) {
    let rank = match j.state {
        JobState::Running | JobState::Held => 0,
        JobState::Queued | JobState::Paused => 1,
        _ => 2,
    };
    // Waiting jobs in the order they will run; finished ones newest first.
    (rank, if rank == 2 { -(j.id as i64) } else { j.id as i64 })
}

enum Act {
    Pause(u64, bool),
    Cancel(u64),
    Retry(u64),
    Remove(u64),
    ClearFinished,
}

pub(crate) fn panel(ui: &mut Ui, app: &mut EditorUi) {
    let t = ui.theme.clone();
    let mut jobs = app.engine.jobs().list();
    jobs.sort_by_key(order);
    let running = jobs.iter().filter(|j| matches!(j.state, JobState::Running | JobState::Held)).count();
    let waiting = jobs.iter().filter(|j| matches!(j.state, JobState::Queued | JobState::Paused)).count();
    let finished = jobs.iter().filter(|j| j.state.finished()).count();
    let mut act = None;

    let head = Layout::row().width(Size::Grow(1.0)).height(Size::Fixed(28.0)).padding(Insets::xy(8.0, 0.0)).gap(8.0).align(Align::Start, Align::Center);
    ui.container(head, Frame::none(), |ui| {
        let summary = match (running, waiting) {
            (0, 0) => "Nothing running".to_string(),
            (r, 0) => format!("{r} running"),
            (0, w) => format!("{w} queued"),
            (r, w) => format!("{r} running · {w} queued"),
        };
        ui.text_with(&summary, t.metrics.font_size, t.palette.text);
        ui.flex();
        if finished > 0 && ui.button_keyed("jobs-clear", "Clear Finished").clicked {
            act = Some(Act::ClearFinished);
        }
    });
    if jobs.is_empty() {
        ui.space(12.0);
        ui.container(Layout::column().width(Size::Grow(1.0)).padding(Insets::xy(12.0, 0.0)).gap(4.0), Frame::none(), |ui| {
            ui.label_muted("No jobs.");
            ui.text_with("Exports (File > Export Media…) and proxies (File > Create Proxies) are queued here and run one after another.", t.metrics.font_size_small, t.palette.text_faint);
        });
        return;
    }

    let opts = ScrollOptions { gap: 4.0, ..ScrollOptions::new(Size::Grow(1.0)) };
    ui.scroll_area_with("jobs-list", opts, |ui| {
        for j in &jobs {
            if let Some(a) = row(ui, j) {
                act = Some(a);
            }
        }
    });

    let q = app.engine.jobs();
    match act {
        Some(Act::Pause(id, on)) => q.pause(id, on),
        Some(Act::Cancel(id)) => q.cancel(id),
        Some(Act::Retry(id)) => q.retry(id),
        Some(Act::Remove(id)) => q.remove(id),
        Some(Act::ClearFinished) => q.clear_finished(),
        None => {}
    }
}

/// One job: what it is, how far along, and its buttons.
fn row(ui: &mut Ui, j: &JobInfo) -> Option<Act> {
    let t = ui.theme.clone();
    let mut act = None;
    let state_color = match &j.state {
        JobState::Running => t.palette.accent,
        JobState::Done => REEL.meter_mid,
        JobState::Failed(_) => REEL.record,
        _ => t.palette.text_muted,
    };
    let frame = Frame { fill: REEL.raised, radius: 4.0, ..Frame::none() };
    let col = Layout::column().width(Size::Grow(1.0)).height(Size::Fit).padding(Insets::xy(10.0, 8.0)).gap(4.0);
    let id = ui.make_id(("job", j.id));
    ui.container_id(id, col, frame, |ui| {
        let line = Layout::row().width(Size::Grow(1.0)).height(Size::Fixed(18.0)).gap(8.0).align(Align::Start, Align::Center);
        ui.container(line, Frame::none(), |ui| {
            ui.text_with(j.kind, t.metrics.font_size_small, t.palette.text_faint);
            ui.text_with(&j.title, t.metrics.font_size, t.palette.text);
            ui.flex();
            ui.text_with(j.state.label(), t.metrics.font_size_small, state_color);
        });
        // What it makes, and for how long it has run.
        let mut detail = j.detail.clone();
        if let Some(d) = j.ran {
            detail = format!("{detail} · {}", duration(d));
        }
        if let JobState::Failed(why) = &j.state {
            detail = format!("{detail} — {why}");
        }
        ui.text_with(&detail, t.metrics.font_size_small, t.palette.text_muted);
        // A bar once it has started (a paused job part-way keeps its bar).
        if matches!(j.state, JobState::Running | JobState::Held) || (j.state == JobState::Paused && j.progress > 0.0) {
            ui.container(Layout::row().width(Size::Grow(1.0)).height(Size::Fixed(18.0)).align(Align::Start, Align::Center), Frame::none(), |ui| {
                // (No label: the bar prints its percentage; its id is scoped by the card.)
                ui.progress("", Some(j.progress));
            });
        }
        let buttons = Layout::row().width(Size::Grow(1.0)).height(Size::Fixed(24.0)).gap(6.0).align(Align::End, Align::Center);
        ui.container(buttons, Frame::none(), |ui| {
            ui.flex();
            match &j.state {
                JobState::Running | JobState::Held | JobState::Queued => {
                    if ui.button_keyed(("job-pause", j.id), "Pause").clicked {
                        act = Some(Act::Pause(j.id, true));
                    }
                    if ui.button_keyed(("job-cancel", j.id), "Cancel").clicked {
                        act = Some(Act::Cancel(j.id));
                    }
                }
                JobState::Paused => {
                    if ui.button_keyed(("job-resume", j.id), "Resume").clicked {
                        act = Some(Act::Pause(j.id, false));
                    }
                    if ui.button_keyed(("job-cancel", j.id), "Cancel").clicked {
                        act = Some(Act::Cancel(j.id));
                    }
                }
                JobState::Failed(_) | JobState::Cancelled => {
                    if ui.button_keyed(("job-retry", j.id), "Retry").clicked {
                        act = Some(Act::Retry(j.id));
                    }
                    if ui.button_keyed(("job-remove", j.id), "Remove").clicked {
                        act = Some(Act::Remove(j.id));
                    }
                }
                JobState::Done => {
                    if ui.button_keyed(("job-remove", j.id), "Remove").clicked {
                        act = Some(Act::Remove(j.id));
                    }
                }
            }
        });
    });
    act
}
