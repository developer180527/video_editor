//! Create Proxies: a small H.264 copy of each chosen clip's picture, made
//! one after another in the background and attached to its asset as it
//! finishes (the "Use Proxies" switch then plays it in place of the
//! original; export always uses the original).
//!
//! Each proxy is an export of a one-clip sequence sized to the proxy and
//! running at the clip's own frame rate, so it inherits the export path:
//! hardware decoding and encoding, upright rotation, square pixels, exact
//! frames, progress and cancelling. Files go to Documents/Proxies.

use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
use std::sync::{Arc, Mutex};
use std::time::Duration;

use ve_model::*;
use ve_plugin_host::Registry;
use ve_ports::{Location, Platform};
use ve_time::{Rate, Time, TimeRange};

use crate::export::{ExportPreset, ExportState};

/// A running Create Proxies, shared with whoever shows its progress.
pub struct ProxyJob {
    /// The assets, by name, in the order they are made.
    pub names: Vec<String>,
    current: AtomicUsize,
    export: Mutex<Option<Arc<ExportState>>>,
    cancel: AtomicBool,
    /// Per asset: the proxy's path, or why it failed.
    results: Mutex<Vec<Result<String, String>>>,
    finished: AtomicBool,
}

impl ProxyJob {
    /// (which one is being made, how many, overall fraction done).
    pub fn progress(&self) -> (usize, usize, f32) {
        let n = self.names.len().max(1);
        let i = self.current.load(Ordering::Relaxed).min(n - 1);
        let within = self.export.lock().unwrap().as_ref().map_or(0.0, |e| e.progress());
        (i, self.names.len(), (i as f32 + within) / n as f32)
    }

    pub fn cancel(&self) {
        self.cancel.store(true, Ordering::Relaxed);
        if let Some(e) = self.export.lock().unwrap().as_ref() {
            e.cancel();
        }
    }

    pub fn cancelled(&self) -> bool {
        self.cancel.load(Ordering::Relaxed)
    }

    /// `Some` when all are done (or it was cancelled): each one's outcome.
    pub fn result(&self) -> Option<Vec<Result<String, String>>> {
        self.finished.load(Ordering::SeqCst).then(|| self.results.lock().unwrap().clone())
    }
}

/// The nearest common rate when `r` is within 0.1% of one (a 23.976 file
/// that stores 879609375/36687041), else `r`.
fn tidy_rate(r: Rate) -> Rate {
    let common = [Rate::FPS_23_976, Rate::FPS_24, Rate::FPS_25, Rate::FPS_29_97, Rate::FPS_30, Rate::FPS_50, Rate::FPS_59_94, Rate::FPS_60, Rate::new(120, 1)];
    common.into_iter().find(|c| ((c.as_f64() - r.as_f64()) / c.as_f64()).abs() < 0.001).unwrap_or(r)
}

/// The proxy's size for a picture shown at `w`×`h`: `height` tall (never
/// larger than the original), the same shape, even sides.
pub fn proxy_size(w: u32, h: u32, height: u32) -> (u32, u32) {
    let ph = height.min(h).max(2);
    let pw = ((w as f64 * ph as f64 / h.max(1) as f64).round() as u32).max(2);
    (pw & !1, ph & !1)
}

/// A project holding just one sequence: `asset` end to end, fitted to the
/// proxy's frame.
fn one_clip(project: &Project, registry: &Registry, asset: &Asset, height: u32) -> Option<Project> {
    let info = asset.info.as_ref()?;
    let v = info.video.as_ref()?;
    let (w, h) = v.display_size();
    let (pw, ph) = proxy_size(w, h, height);
    let mut track = Track::new(TrackKind::Video, "V1");
    let mut seq = Sequence {
        id: SequenceId::new(),
        name: format!("{} proxy", asset.name),
        format: SequenceFormat { width: pw, height: ph, rate: tidy_rate(v.rate), ..SequenceFormat::default() },
        tracks: Default::default(),
        marks: Default::default(),
    };
    seq.tracks.push_back(Arc::new(track.clone()));
    let (_, items) = crate::clips::clips_for_asset(registry, &seq, asset, info.duration, Some(seq.tracks[0].id), None);
    let (_, clip) = items.into_iter().find(|(t, _)| *t == seq.tracks[0].id)?;
    let mut clip = (*clip).clone();
    clip.source_range = TimeRange::new(Time::ZERO, info.duration);
    clip.timeline_start = Time::ZERO;
    track.id = seq.tracks[0].id;
    track.clips.push_back(Arc::new(clip));
    seq.tracks = [Arc::new(track)].into_iter().collect();
    let mut p = project.clone();
    p.sequences.insert(seq.id, Arc::new(seq.clone()));
    p.active_sequence = Some(seq.id);
    Some(p)
}

/// Where `asset`'s proxy goes: Documents/Proxies/<name> Proxy.mp4 (or 2, 3…).
fn target(platform: &Platform, asset: &Asset) -> Option<(MediaRef, String)> {
    let stem = asset.name.rsplit_once('.').map_or(asset.name.as_str(), |(s, _)| s).replace(['/', '\\', ':'], "_");
    (1..1000).find_map(|n| {
        let name = if n == 1 { format!("Proxies/{stem} Proxy.mp4") } else { format!("Proxies/{stem} Proxy {n}.mp4") };
        let r = platform.storage.location(Location::Documents, &name).ok()?;
        let path = platform.storage.resolve_new(&r).ok()?.path?;
        (!path.exists()).then(|| (r, path.display().to_string()))
    })
}

/// Make proxies of `assets` (those with a picture) `height` pixels tall,
/// one after another; `attach` is called with each finished one.
#[allow(clippy::too_many_arguments)]
pub(crate) fn start(
    project: Snapshot,
    platform: Platform,
    registry: Arc<Registry>,
    gpu: (wgpu::Device, wgpu::Queue),
    assets: Vec<AssetId>,
    height: u32,
    importer: Option<Arc<dyn ve_render::TextureImporter>>,
    attach: impl Fn(AssetId, String) + Send + 'static,
) -> Arc<ProxyJob> {
    let chosen: Vec<Arc<Asset>> = assets.iter().filter_map(|id| project.assets.get(id).cloned()).filter(|a| a.info.as_ref().is_some_and(|i| i.video.is_some())).collect();
    let job = Arc::new(ProxyJob {
        names: chosen.iter().map(|a| a.name.clone()).collect(),
        current: AtomicUsize::new(0),
        export: Mutex::new(None),
        cancel: AtomicBool::new(false),
        results: Mutex::new(Vec::new()),
        finished: AtomicBool::new(false),
    });
    let j = job.clone();
    std::thread::Builder::new()
        .name("ve-proxies".into())
        .spawn(move || {
            for (i, asset) in chosen.iter().enumerate() {
                if j.cancelled() {
                    break;
                }
                j.current.store(i, Ordering::Relaxed);
                let r = (|| {
                    let p = one_clip(&project, &registry, asset, height).ok_or("no picture to make a proxy of")?;
                    let (out, path) = target(&platform, asset).ok_or("no place to write the proxy")?;
                    let e = crate::export::start(Arc::new(p), platform.clone(), registry.clone(), gpu.clone(), ExportPreset::Proxy, out, importer.clone());
                    *j.export.lock().unwrap() = Some(e.clone());
                    let r = loop {
                        if let Some(r) = e.result() {
                            break r;
                        }
                        std::thread::sleep(Duration::from_millis(50));
                    };
                    r.map(|()| path)
                })();
                if let Ok(path) = &r {
                    attach(asset.id, path.clone());
                }
                j.results.lock().unwrap().push(r);
            }
            *j.export.lock().unwrap() = None;
            j.finished.store(true, Ordering::SeqCst);
        })
        .expect("proxy thread");
    job
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn sizes_keep_the_shape_and_never_grow() {
        assert_eq!(proxy_size(3840, 2160, 540), (960, 540));
        assert_eq!(proxy_size(1080, 1920, 540), (304, 540), "upright phone video");
        assert_eq!(proxy_size(640, 360, 540), (640, 360), "already small");
        assert_eq!(proxy_size(1998, 1080, 720), (1332, 720));
    }

    #[test]
    fn odd_rates_settle_on_the_common_one() {
        assert_eq!(tidy_rate(Rate::new(879_609_375, 36_687_041)), Rate::FPS_23_976);
        assert_eq!(tidy_rate(Rate::new(30, 1)), Rate::FPS_30);
        assert_eq!(tidy_rate(Rate::new(15, 1)), Rate::new(15, 1));
    }
}
