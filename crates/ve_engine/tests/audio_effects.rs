//! The built-in audio effects as the UI lists them (ve_plugin_host) and as
//! the mixer runs them (ve_media::audio_fx) must agree: every listed effect
//! is one the mixer knows, and every parameter it lists is one the DSP reads.

use ve_engine::{intrinsic, EffectKind, Implementation};
use ve_media::audio_fx;

#[test]
fn listed_audio_effects_are_the_ones_the_mixer_runs() {
    let dsp = include_str!("../../ve_media/src/audio_fx.rs");
    let listed: Vec<_> = intrinsic::all().into_iter().filter(|e| e.kind == EffectKind::AudioFilter).collect();
    let mut ids: Vec<&str> = listed.iter().map(|e| e.plugin.id.as_str()).collect();
    ids.sort();
    let mut known = audio_fx::ALL.to_vec();
    known.sort();
    assert_eq!(ids, known);
    for e in &listed {
        assert!(matches!(e.implementation, Implementation::Mixer), "{}", e.plugin.id);
        for p in &e.params {
            assert!(dsp.contains(&format!("p(\"{}\"", p.id)), "{}: the DSP never reads `{}`", e.plugin.id, p.id);
        }
    }
}
