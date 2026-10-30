//! The job system: long work the editor does in the background — exports
//! and proxies — in one queue, shown in the Jobs panel.
//!
//! - **One at a time, at full speed.** An export already uses every core and
//!   the hardware encoder; two at once would only share them, and finish
//!   both later. Jobs run in the order queued.
//! - **Race to idle.** The runner thread starts when a job is queued and
//!   ends when the queue is empty; nothing waits on a timer. A paused job
//!   sleeps between frames until resumed or cancelled.
//! - **Playback first.** While the editor plays, the running job is held
//!   (between frames), so a background export never makes playback stutter;
//!   it carries on when playback stops.
//! - Each export renders the project as it was when it was queued, as an
//!   export queue does: later edits change later exports.

use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Condvar, Mutex};
use std::time::{Duration, Instant};

use ve_model::{AssetId, MediaRef, Snapshot};
use ve_plugin_host::Registry;
use ve_ports::Platform;

use crate::export::{ExportPreset, ExportState};

pub type JobId = u64;

#[derive(Clone, Debug, PartialEq)]
pub enum JobState {
    Queued,
    Running,
    /// Paused by the user (a queued job can be paused too: it is skipped).
    Paused,
    /// Waiting while the editor plays back.
    Held,
    Done,
    Failed(String),
    Cancelled,
}

impl JobState {
    pub fn finished(&self) -> bool {
        matches!(self, JobState::Done | JobState::Failed(_) | JobState::Cancelled)
    }

    pub fn label(&self) -> &str {
        match self {
            JobState::Queued => "Queued",
            JobState::Running => "Running",
            JobState::Paused => "Paused",
            JobState::Held => "Waiting for playback to stop",
            JobState::Done => "Done",
            JobState::Failed(_) => "Failed",
            JobState::Cancelled => "Cancelled",
        }
    }
}

/// What a job does.
#[derive(Clone)]
pub enum JobKind {
    /// The active sequence of `project` (as queued) to `out`.
    Export { project: Snapshot, preset: ExportPreset, out: MediaRef },
    /// A proxy of `asset`, `height` pixels tall.
    Proxy { project: Snapshot, asset: AssetId, height: u32 },
}

/// One job, as the Jobs panel shows it.
#[derive(Clone, Debug)]
pub struct JobInfo {
    pub id: JobId,
    /// "Export", "Proxy".
    pub kind: &'static str,
    /// What it makes: the file's name, or the clip's.
    pub title: String,
    pub detail: String,
    pub state: JobState,
    /// 0–1 while running or paused part-way; 1 when done.
    pub progress: f32,
    pub queued_at: Instant,
    /// How long it ran (so far, or in all).
    pub ran: Option<Duration>,
}

struct Entry {
    info: JobInfo,
    job: JobKind,
    paused: bool,
    started: Option<Instant>,
    export: Option<Arc<ExportState>>,
}

/// What the runner needs that the queue does not hold per job.
#[derive(Clone)]
pub struct JobContext {
    pub platform: Platform,
    pub registry: Arc<Registry>,
    pub gpu: (wgpu::Device, wgpu::Queue),
    pub importer: Option<Arc<dyn ve_render::TextureImporter>>,
    /// A finished proxy, to attach to its asset: (asset, file path).
    pub attach: Arc<dyn Fn(AssetId, String) + Send + Sync>,
}

#[derive(Default)]
struct Inner {
    entries: Vec<Entry>,
    next: JobId,
    /// A runner thread is alive.
    running: bool,
    ctx: Option<JobContext>,
    /// Bumped on every change, so a UI can tell when to look again.
    version: u64,
}

/// The queue. Cheap to clone; shared by the client and the panel.
#[derive(Clone, Default)]
pub struct Jobs {
    inner: Arc<(Mutex<Inner>, Condvar)>,
    playing: Arc<AtomicBool>,
}

impl Jobs {
    /// Give the runner what it needs (the GPU, plugins, …). Jobs queued
    /// before this wait for it.
    pub fn set_context(&self, ctx: JobContext) {
        let (m, _) = &*self.inner;
        m.lock().unwrap().ctx = Some(ctx);
        self.kick();
    }

    pub fn has_context(&self) -> bool {
        self.inner.0.lock().unwrap().ctx.is_some()
    }

    /// Queue a job; it starts when the ones before it are done.
    pub fn add(&self, title: String, detail: String, job: JobKind) -> JobId {
        let (m, _) = &*self.inner;
        let mut g = m.lock().unwrap();
        g.next += 1;
        let id = g.next;
        let kind = match job {
            JobKind::Export { .. } => "Export",
            JobKind::Proxy { .. } => "Proxy",
        };
        g.entries.push(Entry {
            info: JobInfo { id, kind, title, detail, state: JobState::Queued, progress: 0.0, queued_at: Instant::now(), ran: None },
            job,
            paused: false,
            started: None,
            export: None,
        });
        g.version += 1;
        drop(g);
        self.kick();
        id
    }

    /// Every job, oldest first, with live progress.
    pub fn list(&self) -> Vec<JobInfo> {
        let g = self.inner.0.lock().unwrap();
        g.entries
            .iter()
            .map(|e| {
                let mut i = e.info.clone();
                if let Some(x) = &e.export {
                    if !i.state.finished() {
                        i.progress = x.progress();
                    }
                }
                if let (Some(t), false) = (e.started, i.state.finished()) {
                    i.ran = Some(t.elapsed());
                }
                i
            })
            .collect()
    }

    /// Changes whenever a job is added, starts, ends or changes state.
    pub fn version(&self) -> u64 {
        self.inner.0.lock().unwrap().version
    }

    /// Whether anything is queued, running, paused or held.
    pub fn active(&self) -> bool {
        self.inner.0.lock().unwrap().entries.iter().any(|e| !e.info.state.finished())
    }

    pub fn pause(&self, id: JobId, on: bool) {
        let (m, _) = &*self.inner;
        let mut g = m.lock().unwrap();
        let held = self.playing.load(Ordering::Relaxed);
        if let Some(e) = g.entries.iter_mut().find(|e| e.info.id == id && !e.info.state.finished()) {
            e.paused = on;
            if let Some(x) = &e.export {
                x.set_paused(on);
            }
            e.info.state = match (on, &e.export, held) {
                (true, _, _) => JobState::Paused,
                (false, Some(_), true) => JobState::Held,
                (false, Some(_), false) => JobState::Running,
                (false, None, _) => JobState::Queued,
            };
        }
        g.version += 1;
        drop(g);
        self.kick();
    }

    pub fn cancel(&self, id: JobId) {
        let (m, _) = &*self.inner;
        let mut g = m.lock().unwrap();
        if let Some(e) = g.entries.iter_mut().find(|e| e.info.id == id && !e.info.state.finished()) {
            match &e.export {
                // The runner records the outcome when the export stops.
                Some(x) => x.cancel(),
                None => e.info.state = JobState::Cancelled,
            }
        }
        g.version += 1;
    }

    /// Queue a finished (failed, cancelled or done) job again.
    pub fn retry(&self, id: JobId) {
        let (m, _) = &*self.inner;
        let mut g = m.lock().unwrap();
        if let Some(e) = g.entries.iter_mut().find(|e| e.info.id == id && e.info.state.finished()) {
            e.info.state = JobState::Queued;
            e.info.progress = 0.0;
            e.info.ran = None;
            e.paused = false;
            e.started = None;
            e.export = None;
        }
        g.version += 1;
        drop(g);
        self.kick();
    }

    /// Drop a finished job from the list.
    pub fn remove(&self, id: JobId) {
        let mut g = self.inner.0.lock().unwrap();
        g.entries.retain(|e| e.info.id != id || !e.info.state.finished());
        g.version += 1;
    }

    pub fn clear_finished(&self) {
        let mut g = self.inner.0.lock().unwrap();
        g.entries.retain(|e| !e.info.state.finished());
        g.version += 1;
    }

    /// The editor started or stopped playing: hold the running job meanwhile.
    pub fn set_playing(&self, on: bool) {
        if self.playing.swap(on, Ordering::Relaxed) == on {
            return;
        }
        let mut g = self.inner.0.lock().unwrap();
        for e in g.entries.iter_mut().filter(|e| e.export.is_some() && !e.info.state.finished()) {
            if let Some(x) = &e.export {
                x.set_held(on);
            }
            if !e.paused {
                e.info.state = if on { JobState::Held } else { JobState::Running };
            }
        }
        g.version += 1;
    }

    /// Wait (blocking, no timer) until nothing is queued or running. For
    /// tools and tests.
    pub fn wait_idle(&self) {
        let (m, cv) = &*self.inner;
        let g = m.lock().unwrap();
        let _g = cv.wait_while(g, |g| g.running || g.entries.iter().any(|e| matches!(e.info.state, JobState::Queued | JobState::Running | JobState::Held))).unwrap();
    }

    /// Start the runner if there is work and it is not running.
    fn kick(&self) {
        let (m, _) = &*self.inner;
        let mut g = m.lock().unwrap();
        let ready = g.ctx.is_some() && g.entries.iter().any(|e| e.info.state == JobState::Queued && !e.paused);
        if g.running || !ready {
            return;
        }
        g.running = true;
        drop(g);
        let me = self.clone();
        std::thread::Builder::new().name("ve-jobs".into()).spawn(move || me.run()).expect("jobs thread");
    }

    /// The runner: the next queued job, until there are none; then it ends.
    fn run(&self) {
        let (m, cv) = &*self.inner;
        loop {
            let (id, job, ctx) = {
                let mut g = m.lock().unwrap();
                let ctx = g.ctx.clone();
                let next = g.entries.iter().position(|e| e.info.state == JobState::Queued && !e.paused);
                match (next, ctx) {
                    (Some(i), Some(ctx)) => {
                        let e = &mut g.entries[i];
                        e.started = Some(Instant::now());
                        e.info.state = JobState::Running;
                        let job = e.job.clone();
                        let id = e.info.id;
                        g.version += 1;
                        (id, job, ctx)
                    }
                    _ => {
                        // Race to idle: no work, no thread.
                        g.running = false;
                        g.version += 1;
                        cv.notify_all();
                        return;
                    }
                }
            };
            let outcome = self.run_one(id, job, &ctx);
            let mut g = m.lock().unwrap();
            if let Some(e) = g.entries.iter_mut().find(|e| e.info.id == id) {
                e.info.ran = e.started.map(|t| t.elapsed());
                match outcome {
                    Ok(detail) => {
                        e.info.state = JobState::Done;
                        e.info.progress = 1.0;
                        if let Some(d) = detail {
                            e.info.detail = d;
                        }
                    }
                    Err(err) if err == "cancelled" => e.info.state = JobState::Cancelled,
                    Err(err) => e.info.state = JobState::Failed(err),
                }
                e.export = None;
            }
            g.version += 1;
            cv.notify_all();
        }
    }

    /// Run one job to its end. `Ok(Some(detail))` replaces the job's detail.
    fn run_one(&self, id: JobId, job: JobKind, ctx: &JobContext) -> Result<Option<String>, String> {
        let (project, preset, out, attach) = match job {
            JobKind::Export { project, preset, out } => (project, preset, out, None),
            JobKind::Proxy { project, asset, height } => {
                let (p, out, path) = crate::proxy::prepare(&project, &ctx.registry, &ctx.platform, asset, height)?;
                (Arc::new(p), ExportPreset::Proxy, out, Some((asset, path)))
            }
        };
        let e = crate::export::start(project, ctx.platform.clone(), ctx.registry.clone(), ctx.gpu.clone(), preset, out, ctx.importer.clone());
        {
            let mut g = self.inner.0.lock().unwrap();
            if let Some(entry) = g.entries.iter_mut().find(|x| x.info.id == id) {
                // Paused or cancelled before it started, or the editor is playing.
                if entry.paused {
                    e.set_paused(true);
                }
                if self.playing.load(Ordering::Relaxed) {
                    e.set_held(true);
                    if !entry.paused {
                        entry.info.state = JobState::Held;
                    }
                }
                entry.export = Some(e.clone());
            }
        }
        e.wait()?;
        Ok(match attach {
            Some((asset, path)) => {
                (ctx.attach)(asset, path.clone());
                Some(path)
            }
            None => None,
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn states_say_whether_they_are_over() {
        assert!(JobState::Done.finished() && JobState::Cancelled.finished() && JobState::Failed("x".into()).finished());
        assert!(!JobState::Queued.finished() && !JobState::Held.finished() && !JobState::Paused.finished());
    }

    #[test]
    fn without_a_context_jobs_wait_and_no_thread_starts() {
        let jobs = Jobs::default();
        let p: Snapshot = Arc::new(ve_model::Project::new("t"));
        let id = jobs.add("a.mp4".into(), String::new(), JobKind::Export { project: p, preset: ExportPreset::H264Mp4, out: MediaRef("file:/x".into()) });
        assert!(!jobs.inner.0.lock().unwrap().running, "nothing runs without the GPU");
        assert_eq!(jobs.list()[0].state, JobState::Queued);
        jobs.pause(id, true);
        assert_eq!(jobs.list()[0].state, JobState::Paused);
        jobs.cancel(id);
        assert_eq!(jobs.list()[0].state, JobState::Cancelled, "a job that never started cancels at once");
        jobs.clear_finished();
        assert!(jobs.list().is_empty());
    }
}
