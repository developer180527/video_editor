//! Create Proxies: a small H.264 copy of each chosen clip's picture, made
//! as a job (see `jobs`) and attached to its asset when it (the "Use Proxies" switch then plays it in place of the
//! original; export always uses the original).
//!
//! Each proxy is an export of a one-clip sequence sized to the proxy and
//! running at the clip's own frame rate, so it inherits the export path:
//! hardware decoding and encoding, upright rotation, square pixels, exact
//! frames, progress and cancelling. Files go to Documents/Proxies.

use std::sync::Arc;

use ve_model::*;
use ve_plugin_host::Registry;
use ve_ports::{Location, Platform};
use ve_time::{Rate, Time, TimeRange};


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

/// What making `asset`'s proxy exports: a one-clip project, where to write
/// it, and that file's path (to attach when done).
pub(crate) fn prepare(project: &Project, registry: &Registry, platform: &Platform, asset: AssetId, height: u32) -> Result<(Project, MediaRef, String), String> {
    let a = project.assets.get(&asset).ok_or("the clip is no longer in the project")?;
    let p = one_clip(project, registry, a, height).ok_or("no picture to make a proxy of")?;
    let (out, path) = target(platform, a).ok_or("no place to write the proxy")?;
    Ok((p, out, path))
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
