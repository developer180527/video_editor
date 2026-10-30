//! The job system: long work the editor does in the background — exports
//! and proxies — in one queue, shown in the Jobs panel.
//!
//! - **One at a time, at full speed.** Jobs run in the order queued. (Two
//!   exports side by side share the decoders and encoders an export is
//!   limited by — `ve bench --jobs` measures it — and finish both later.)
//! - **Race to idle.** The runner thread starts when a job can run and ends
//!   when none can; nothing waits on a timer. A paused job sleeps between
//!   frames until resumed or cancelled.
//! - **Pausing yields.** A job paused part-way gives up its turn: the next
//!   one runs. Resumed, it waits for its turn again and carries on from
//!   where it stopped.
//! - **Playback first.** While the editor plays, the running job is held
//!   (between frames), so a background export never makes playback stutter;
//!   it carries on when playback stops. The engine says when it plays.
//! - Each job renders the project — and the plugins — as they were when it
//!   was queued, as an export queue does: later edits change later exports.

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
    /// Waiting for its turn (a resumed job too, part-way through).
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

/// What a job does, with what it was queued with.
#[derive(Clone)]
pub enum JobKind {
    /// The active sequence of `project` (as queued) to `out`.
    Export { project: Snapshot, plugins: Arc<Registry>, preset: ExportPreset, out: MediaRef },
    /// A proxy of `asset`, `height` pixels tall.
    Proxy { project: Snapshot, plugins: Arc<Registry>, asset: AssetId, height: u32 },
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
    /// How long it has run (paused stretches not counted).
    pub ran: Option<Duration>,
}

struct Entry {
    info: JobInfo,
    job: JobKind,
    paused: bool,
    /// Since when it has been running, this stretch.
    since: Option<Instant>,
    /// Run time before this stretch (it was paused part-way).
    ran: Duration,
    /// Its export, once started; kept while paused part-way.
    export: Option<Arc<ExportState>>,
}

impl Entry {
    fn ran(&self) -> Option<Duration> {
        let now = self.since.map(|t| t.elapsed());
        (now.is_some() || self.ran > Duration::ZERO).then(|| self.ran + now.unwrap_or_default())
    }

    /// Stop the clock (paused, or over).
    fn stop_clock(&mut self) {
        if let Some(t) = self.since.take() {
            self.ran += t.elapsed();
        }
        self.info.ran = self.ran();
    }
}

/// What the runner needs that the queue does not hold per job.
#[derive(Clone)]
pub struct JobContext {
    pub platform: Platform,
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
    /// The job the runner is on (waiting for it to end or be paused).
    current: Option<JobId>,
    ctx: Option<JobContext>,
    /// Bumped on every change, so a UI can tell when to look again.
    version: u64,
}

/// The queue. Cheap to clone; shared by the client and the panel.
#[derive(Clone, Default)]
pub struct Jobs {
    inner: Arc<(Mutex<Inner>, Condvar)>,
    playing: Arc<AtomicBool>,
    waker: Arc<Mutex<Option<crate::Waker>>>,
}

impl Jobs {
    /// Called on every change (a job added, started, paused, finished…),
    /// so a UI that sleeps when idle wakes to show it.
    pub fn set_waker(&self, w: crate::Waker) {
        *self.waker.lock().unwrap() = Some(w);
    }

    /// Give the runner what it needs (the GPU, …). Jobs queued before this
    /// wait for it.
    pub fn set_context(&self, ctx: JobContext) {
        let mut g = self.inner.0.lock().unwrap();
        g.ctx = Some(ctx);
        self.kick(g);
    }

    pub fn has_context(&self) -> bool {
        self.inner.0.lock().unwrap().ctx.is_some()
    }

    /// Queue a job; it starts when the ones before it are done.
    pub fn add(&self, title: String, detail: String, job: JobKind) -> JobId {
        let mut g = self.inner.0.lock().unwrap();
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
            since: None,
            ran: Duration::ZERO,
            export: None,
        });
        self.kick(g);
        id
    }

    /// Every job, oldest first, with live progress.
    pub fn list(&self) -> Vec<JobInfo> {
        let g = self.inner.0.lock().unwrap();
        g.entries
            .iter()
            .map(|e| {
                let mut i = e.info.clone();
                if let (Some(x), false) = (&e.export, i.state.finished()) {
                    i.progress = x.progress();
                }
                if !i.state.finished() {
                    i.ran = e.ran();
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

    /// Whether a job is making progress now (worth redrawing its bar for).
    pub fn running(&self) -> bool {
        self.inner.0.lock().unwrap().entries.iter().any(|e| e.info.state == JobState::Running)
    }

    pub fn pause(&self, id: JobId, on: bool) {
        let mut g = self.inner.0.lock().unwrap();
        let held = self.playing.load(Ordering::Relaxed);
        let current = g.current == Some(id);
        let Some(e) = g.entries.iter_mut().find(|e| e.info.id == id && !e.info.state.finished()) else { return };
        if e.paused == on {
            return;
        }
        e.paused = on;
        if on {
            // Gated at once; the runner sees it paused and moves on.
            if let Some(x) = &e.export {
                x.set_paused(true);
            }
            e.stop_clock();
            e.info.state = JobState::Paused;
        } else if !current {
            // Resumed: back in line. The runner lifts the gate on its turn.
            e.info.state = JobState::Queued;
        } else {
            // Paused and resumed before the runner noticed: it never yielded.
            if let Some(x) = &e.export {
                x.set_paused(false);
            }
            e.since = Some(Instant::now());
            e.info.state = if held { JobState::Held } else { JobState::Running };
        }
        self.kick(g);
    }

    pub fn cancel(&self, id: JobId) {
        let mut g = self.inner.0.lock().unwrap();
        let Some(e) = g.entries.iter_mut().find(|e| e.info.id == id && !e.info.state.finished()) else { return };
        match &e.export {
            // Its thread stops between frames, removes the partial file and
            // records the outcome.
            Some(x) => x.cancel(),
            // Not started (or still being prepared): over now. The runner
            // checks before it starts the export.
            None => {
                e.stop_clock();
                e.info.state = JobState::Cancelled;
            }
        }
        self.changed(&mut g);
    }

    /// Queue a finished (failed, cancelled or done) job again.
    pub fn retry(&self, id: JobId) {
        let mut g = self.inner.0.lock().unwrap();
        let Some(e) = g.entries.iter_mut().find(|e| e.info.id == id && e.info.state.finished()) else { return };
        e.info.state = JobState::Queued;
        e.info.progress = 0.0;
        e.info.ran = None;
        e.paused = false;
        e.since = None;
        e.ran = Duration::ZERO;
        e.export = None;
        self.kick(g);
    }

    /// Drop a finished job from the list.
    pub fn remove(&self, id: JobId) {
        let mut g = self.inner.0.lock().unwrap();
        g.entries.retain(|e| e.info.id != id || !e.info.state.finished());
        self.changed(&mut g);
    }

    pub fn clear_finished(&self) {
        let mut g = self.inner.0.lock().unwrap();
        g.entries.retain(|e| !e.info.state.finished());
        self.changed(&mut g);
    }

    /// The editor started or stopped playing: hold the running job meanwhile.
    /// The engine client calls this whenever its transport changes.
    pub fn set_playing(&self, on: bool) {
        if self.playing.swap(on, Ordering::Relaxed) == on {
            return;
        }
        let mut g = self.inner.0.lock().unwrap();
        for e in g.entries.iter_mut().filter(|e| !e.info.state.finished()) {
            if let Some(x) = &e.export {
                x.set_held(on);
            }
            match (&e.info.state, on) {
                (JobState::Running, true) => e.info.state = JobState::Held,
                (JobState::Held, false) => e.info.state = JobState::Running,
                _ => {}
            }
        }
        self.changed(&mut g);
    }

    /// Wait (blocking, no timer) until no job can go on without the user:
    /// each is finished or paused. For tools and tests.
    pub fn wait_idle(&self) {
        let (m, cv) = &*self.inner;
        let g = m.lock().unwrap();
        let _g = cv.wait_while(g, |g| g.entries.iter().any(|e| !e.info.state.finished() && !e.paused)).unwrap();
    }

    /// Something changed: tell whoever waits or shows it.
    fn changed(&self, g: &mut Inner) {
        g.version += 1;
        self.inner.1.notify_all();
        if let Some(w) = self.waker.lock().unwrap().as_ref() {
            w();
        }
    }

    /// After a change: start the runner if there is a job it can run and it
    /// is not running.
    fn kick(&self, mut g: std::sync::MutexGuard<Inner>) {
        self.changed(&mut g);
        let ready = g.ctx.is_some() && g.entries.iter().any(|e| e.info.state == JobState::Queued && !e.paused);
        if g.running || !ready {
            return;
        }
        g.running = true;
        drop(g);
        let me = self.clone();
        std::thread::Builder::new().name("ve-jobs".into()).spawn(move || me.run()).expect("jobs thread");
    }

    /// The runner: the next job in line, until there are none; then it ends.
    fn run(&self) {
        let (m, cv) = &*self.inner;
        loop {
            let mut g = m.lock().unwrap();
            let next = g.entries.iter().position(|e| e.info.state == JobState::Queued && !e.paused);
            let (Some(i), Some(ctx)) = (next, g.ctx.clone()) else {
                // Race to idle: no work, no thread.
                g.running = false;
                self.changed(&mut g);
                return;
            };
            let held = self.playing.load(Ordering::Relaxed);
            g.current = next.map(|i| g.entries[i].info.id);
            let e = &mut g.entries[i];
            let id = e.info.id;
            e.since = Some(Instant::now());
            e.info.state = if held { JobState::Held } else { JobState::Running };
            match e.export.clone() {
                // Its turn again after a pause: lift the gate.
                Some(x) => {
                    x.set_held(held);
                    x.set_paused(false);
                    self.changed(&mut g);
                }
                None => {
                    let job = e.job.clone();
                    self.changed(&mut g);
                    drop(g);
                    if let Err(err) = self.start(id, job, &ctx) {
                        self.finish(id, Err(err), None);
                    }
                    g = m.lock().unwrap();
                }
            }
            // Until it is over or paused (then the next one runs).
            let mut g = cv.wait_while(g, |g| g.entries.iter().find(|e| e.info.id == id).is_some_and(|e| !e.info.state.finished() && !e.paused)).unwrap();
            g.current = None;
        }
    }

    /// Start job `id`'s export; its outcome is recorded when it ends.
    fn start(&self, id: JobId, job: JobKind, ctx: &JobContext) -> Result<(), String> {
        let (project, plugins, preset, out, proxy) = match job {
            JobKind::Export { project, plugins, preset, out } => (project, plugins, preset, out, None),
            JobKind::Proxy { project, plugins, asset, height } => {
                let (p, out, path) = crate::proxy::prepare(&project, &plugins, &ctx.platform, asset, height)?;
                (Arc::new(p), plugins, ExportPreset::Proxy, out, Some((asset, path)))
            }
        };
        let x = ExportState::new(&project, &ctx.platform, &out);
        {
            let mut g = self.inner.0.lock().unwrap();
            let Some(e) = g.entries.iter_mut().find(|e| e.info.id == id) else { return Ok(()) };
            // Cancelled while being prepared: nothing is made.
            if e.info.state.finished() {
                return Ok(());
            }
            // Paused while being prepared, or the editor is playing: it starts
            // gated. From here on pause, hold and cancel reach it directly.
            x.set_paused(e.paused);
            x.set_held(self.playing.load(Ordering::Relaxed));
            e.export = Some(x.clone());
        }
        let (me, attach) = (self.clone(), ctx.attach.clone());
        let done = {
            let x = x.clone();
            move || {
                let r = x.result().unwrap_or(Err("the export ended without a result".into()));
                let detail = match (&r, proxy) {
                    (Ok(()), Some((asset, path))) => {
                        attach(asset, path.clone());
                        Some(path)
                    }
                    _ => None,
                };
                me.finish(id, r, detail);
            }
        };
        crate::export::launch(x, project, ctx.platform.clone(), plugins, ctx.gpu.clone(), preset, out, ctx.importer.clone(), Box::new(done));
        Ok(())
    }

    /// Record how job `id` ended. `detail` replaces its detail.
    fn finish(&self, id: JobId, r: Result<(), String>, detail: Option<String>) {
        let mut g = self.inner.0.lock().unwrap();
        if let Some(e) = g.entries.iter_mut().find(|e| e.info.id == id) {
            e.stop_clock();
            match r {
                Ok(()) => {
                    e.info.state = JobState::Done;
                    e.info.progress = 1.0;
                    if let Some(d) = detail {
                        e.info.detail = d;
                    }
                }
                Err(err) if err == "cancelled" => e.info.state = JobState::Cancelled,
                Err(err) => e.info.state = JobState::Failed(err),
            }
            // Its frames, decoders and encoder are gone with its thread.
            e.export = None;
        }
        self.changed(&mut g);
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
        let id = jobs.add("a.mp4".into(), String::new(), JobKind::Export { project: p, plugins: Arc::new(Registry::default()), preset: ExportPreset::H264Mp4, out: MediaRef("file:/x".into()) });
        assert!(!jobs.inner.0.lock().unwrap().running, "nothing runs without the GPU");
        assert_eq!(jobs.list()[0].state, JobState::Queued);
        jobs.pause(id, true);
        assert_eq!(jobs.list()[0].state, JobState::Paused);
        jobs.cancel(id);
        assert_eq!(jobs.list()[0].state, JobState::Cancelled, "a job that never started cancels at once");
        jobs.clear_finished();
        assert!(jobs.list().is_empty());
    }

    #[test]
    fn every_change_wakes_the_ui_and_waiting_jobs_ignore_playback() {
        let jobs = Jobs::default();
        let wakes = Arc::new(std::sync::atomic::AtomicUsize::new(0));
        let w = wakes.clone();
        jobs.set_waker(Arc::new(move || {
            w.fetch_add(1, Ordering::SeqCst);
        }));
        let p: Snapshot = Arc::new(ve_model::Project::new("t"));
        let id = jobs.add("a.mp4".into(), String::new(), JobKind::Export { project: p, plugins: Arc::new(Registry::default()), preset: ExportPreset::H264Mp4, out: MediaRef("file:/x".into()) });
        assert!(wakes.load(Ordering::SeqCst) >= 1, "queued: the UI is told");
        let before = wakes.load(Ordering::SeqCst);
        jobs.set_playing(true);
        assert_eq!(jobs.list()[0].state, JobState::Queued, "only a started job is held");
        jobs.pause(id, true);
        assert!(!jobs.running(), "nothing to redraw for");
        // Only paused jobs left: nothing more can happen without the user.
        jobs.wait_idle();
        jobs.pause(id, false);
        assert_eq!(jobs.list()[0].state, JobState::Queued, "resumed: back in line");
        assert!(wakes.load(Ordering::SeqCst) >= before + 3);
    }
}
