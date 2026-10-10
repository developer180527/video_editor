//! Making clips the way the editor wants them: with their intrinsic effects
//! (Motion, Opacity / Volume, Panner) already attached and initialised.

use std::sync::Arc;
use ve_model::*;
use ve_plugin_host::{intrinsic, Registry};
use ve_time::{Time, TimeRange};

/// A clip of `asset` for a track of `kind`, `duration` long from the source's
/// start, with intrinsic effects at their defaults for `format`.
pub fn make_clip(
    registry: &Registry,
    format: &SequenceFormat,
    asset: &Asset,
    kind: TrackKind,
    duration: Time,
    link: Option<LinkId>,
) -> Clip {
    let suffix = match kind {
        TrackKind::Video => " [V]",
        TrackKind::Audio => " [A]",
    };
    let size = asset.info.as_ref().and_then(|i| i.video.as_ref()).map(|v| v.display_size()).unwrap_or((format.width, format.height));
    clip_of(
        ClipSource::Asset { asset: asset.id, audio_stream: 0 },
        format!("{}{suffix}", asset.name),
        intrinsics(registry, format, kind, size),
        duration,
        link,
    )
}

/// A generator clip (`ve.color`, `ve.bars`, `ve.title`, or a plugin's),
/// `duration` long: its parameters as an effect at their defaults (so they
/// animate and undo like any effect's), then Motion and Opacity.
pub fn make_generator_clip(registry: &Registry, format: &SequenceFormat, plugin: &PluginRef, duration: Time) -> Option<Clip> {
    let info = registry.find(plugin)?;
    let params = info.params.iter().map(|p| (p.id.clone(), Param::Constant(default_value(&p.kind, p.default)))).collect();
    let mut effects = Vector::new();
    effects.push_back(Arc::new(Effect { id: EffectId::new(), plugin: plugin.clone(), enabled: true, params }));
    effects.append(intrinsics(registry, format, TrackKind::Video, (format.width, format.height)));
    Some(clip_of(ClipSource::Generator { plugin: plugin.clone() }, info.name.clone(), effects, duration, None))
}

/// A clip of the whole of `sequence` (a compound or multicam clip) for a
/// track of `kind`.
pub fn make_sequence_clip(registry: &Registry, format: &SequenceFormat, sequence: &Sequence, kind: TrackKind, link: Option<LinkId>) -> Clip {
    let size = (sequence.format.width, sequence.format.height);
    clip_of(
        ClipSource::Sequence { sequence: sequence.id, angle: None },
        sequence.name.clone(),
        intrinsics(registry, format, kind, size),
        sequence.duration().max(Time(1)),
        link,
    )
}

/// The intrinsic effects for a clip of `kind` whose picture is `size`:
/// Motion (fitted to the frame, like Premiere's "Set to Frame Size") and
/// Opacity on video; Volume and Panner on audio.
fn intrinsics(registry: &Registry, format: &SequenceFormat, kind: TrackKind, size: (u32, u32)) -> Vector<Arc<Effect>> {
    let ids: &[&str] = match kind {
        TrackKind::Video => &[intrinsic::MOTION, intrinsic::OPACITY],
        TrackKind::Audio => &[intrinsic::VOLUME, intrinsic::PANNER],
    };
    let (sw, sh) = (size.0 as f64, size.1 as f64);
    ids.iter()
        .filter_map(|id| registry.find(&intrinsic::plugin_ref(id)))
        .map(|info| {
            let params = info
                .params
                .iter()
                .map(|p| {
                    let fit = (format.width as f64 / sw).min(format.height as f64 / sh) * 100.0;
                    let d = match p.id.as_str() {
                        "scale" => [(fit * 100.0).round() / 100.0, 0.0, 0.0, 0.0],
                        "position" => [format.width as f64 / 2.0, format.height as f64 / 2.0, 0.0, 0.0],
                        "anchor" => [sw / 2.0, sh / 2.0, 0.0, 0.0],
                        _ => p.default,
                    };
                    (p.id.clone(), Param::Constant(default_value(&p.kind, d)))
                })
                .collect();
            Arc::new(Effect { id: EffectId::new(), plugin: info.plugin.clone(), enabled: true, params })
        })
        .collect()
}

fn clip_of(source: ClipSource, name: String, effects: Vector<Arc<Effect>>, duration: Time, link: Option<LinkId>) -> Clip {
    Clip {
        id: ClipId::new(),
        name,
        source,
        source_range: TimeRange::new(Time::ZERO, duration),
        timeline_start: Time::ZERO,
        enabled: true,
        link,
        effects,
        retime: Default::default(),
        transition_in: None,
        transition_out: None,
        channels: Vec::new(),
    }
}

/// Everything a drop of `asset` puts on the timeline: its picture (if any)
/// on `video_track`, and one clip per audio stream on consecutive audio
/// tracks from `first_audio` (the first audio track when `None`) — all
/// linked, `duration` long from the source's start. Cameras that record each
/// channel as its own stream (eight mono streams) land on A1…A8, like any
/// professional editor. Where the sequence has too few audio tracks, the
/// returned commands add them; apply those first.
pub fn clips_for_asset(
    registry: &Registry,
    seq: &Sequence,
    asset: &Asset,
    duration: Time,
    video_track: Option<TrackId>,
    first_audio: Option<TrackId>,
) -> (Vec<crate::Command>, Vec<(TrackId, Arc<Clip>)>) {
    let Some(info) = &asset.info else { return (vec![], vec![]) };
    let streams = info.audio.len();
    let link = (info.video.is_some() as usize + streams > 1).then(LinkId::new);
    let mut items = Vec::new();
    if let (Some(_), Some(t)) = (&info.video, video_track.or_else(|| seq.tracks.iter().find(|t| t.kind == TrackKind::Video).map(|t| t.id))) {
        items.push((t, Arc::new(make_clip(registry, &seq.format, asset, TrackKind::Video, duration, link))));
    }
    // Audio tracks from the first one wanted, then new ones after the last.
    let audio: Vec<TrackId> = seq.tracks.iter().filter(|t| t.kind == TrackKind::Audio).map(|t| t.id).collect();
    let from = first_audio.and_then(|f| audio.iter().position(|t| *t == f)).unwrap_or(0);
    let mut tracks: Vec<TrackId> = audio.iter().skip(from).take(streams).copied().collect();
    let mut adds = Vec::new();
    while tracks.len() < streams {
        let n = audio.len() + adds.len() + 1;
        let track = Track::new(TrackKind::Audio, format!("A{n}"));
        tracks.push(track.id);
        adds.push(crate::Command::AddTrack { sequence: seq.id, index: seq.tracks.len() + adds.len(), track: Arc::new(track) });
    }
    for (k, t) in tracks.into_iter().enumerate() {
        let mut clip = make_clip(registry, &seq.format, asset, TrackKind::Audio, duration, link);
        clip.source = ClipSource::Asset { asset: asset.id, audio_stream: k as u32 };
        if streams > 1 {
            clip.name = format!("{} [A{}]", asset.name, k + 1);
        }
        items.push((t, Arc::new(clip)));
    }
    (adds, items)
}

/// A parameter's value from its declared kind and four numbers.
pub fn default_value(kind: &ve_plugin_host::ParamKind, d: [f64; 4]) -> Value {
    use ve_plugin_host::ParamKind::*;
    match kind {
        Bool => Value::Bool(d[0] != 0.0),
        Int => Value::Int(d[0] as i64),
        Float => Value::Float(d[0]),
        Vec2 => Value::Vec2([d[0], d[1]]),
        Color => Value::Color(d.map(|x| x as f32)),
        Choice(_) => Value::Choice(d[0] as u32),
        Text(t) => Value::Text(t.clone()),
    }
}
