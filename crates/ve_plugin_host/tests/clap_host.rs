//! The CLAP host against a real plugin file: `clap_test_gain`, built in
//! this workspace (a gain with a polarity choice that asserts it is called
//! on the right threads).

use std::sync::Arc;

use ve_model::{PluginApi, PluginRef};
use ve_plugin_host::clap::ClapHost;
use ve_plugin_host::{EffectKind, ParamKind};
use ve_ports::AudioPlugins;

fn plugin_file() -> String {
    // Tests run from target/<profile>/deps; the library sits one level up.
    let deps = std::env::current_exe().unwrap().parent().unwrap().to_path_buf();
    let name = format!("{}clap_test_gain{}", std::env::consts::DLL_PREFIX, std::env::consts::DLL_SUFFIX);
    [deps.join(&name), deps.parent().unwrap().join(&name)].into_iter().find(|p| p.exists()).expect("clap_test_gain built").display().to_string()
}

fn host() -> ClapHost {
    let libs = platform_desktop::DesktopLibraries { linked: vec![] };
    let host = ClapHost::scan_paths(&libs, &[plugin_file()]);
    assert!(host.problems.is_empty(), "{:?}", host.problems);
    host
}

#[test]
fn scanning_lists_the_effect_and_its_parameters() {
    let host = host();
    let fx = host.effects();
    assert_eq!(fx.len(), 1);
    let e = &fx[0];
    assert_eq!((e.plugin.api, e.plugin.id.as_str(), e.name.as_str()), (PluginApi::Clap, "org.ve.test.gain", "Test Gain"));
    assert_eq!(e.kind, EffectKind::AudioFilter);
    assert_eq!(e.category, "CLAP · ve tests");
    assert_eq!(e.params.len(), 2);
    assert_eq!((e.params[0].id.as_str(), e.params[0].label.as_str(), e.params[0].kind.clone()), ("7", "Gain", ParamKind::Float));
    assert_eq!((e.params[0].min, e.params[0].max), (-24.0, 24.0));
    assert!(e.params[0].animatable);
    assert_eq!(e.params[1].kind, ParamKind::Choice(vec!["Normal".into(), "Inverted".into()]), "a stepped parameter, named by the plugin");
}

#[test]
fn an_instance_processes_with_its_parameters_on_the_callers_thread() {
    let host = Arc::new(host());
    let r = PluginRef { api: PluginApi::Clap, id: "org.ve.test.gain".into(), major_version: 1 };
    // As the mixer does: instantiate and process from its own thread.
    let h = host.clone();
    std::thread::spawn(move || {
        let mut fx = h.instantiate(&r, 48_000).unwrap();
        let (mut l, mut r) = (vec![1.0f32; 10_000], vec![0.5f32; 10_000]);
        fx.process(&[("7", -6.0206), ("9", 0.0)], &mut l, &mut r);
        assert!((l[9_999] - 0.5).abs() < 1e-3 && (r[0] - 0.25).abs() < 1e-3, "-6 dB on both sides, past one 4096-frame slice: {} {}", l[9_999], r[0]);
        // The choice's index becomes the plugin's value.
        let (mut l, mut r) = (vec![1.0f32; 64], vec![1.0f32; 64]);
        fx.process(&[("7", 0.0), ("9", 1.0)], &mut l, &mut r);
        assert_eq!((l[0], r[63]), (-1.0, -1.0));
        fx.reset();
        drop(fx);
    })
    .join()
    .unwrap();
    assert!(host.instantiate(&PluginRef { api: PluginApi::Clap, id: "nope".into(), major_version: 1 }, 48_000).is_err());
    // Give the main thread a moment to destroy the instance (it asserts it is the main thread).
    std::thread::sleep(std::time::Duration::from_millis(100));
}

/// A macOS-style bundle (`Name.clap/Contents/MacOS/Name`) loads like a file.
/// (Bundles are macOS's; elsewhere a .clap is a plain library.)
#[cfg(target_os = "macos")]
#[test]
fn a_bundle_loads_like_a_file() {
    let dir = std::path::PathBuf::from(env!("CARGO_TARGET_TMPDIR")).join("Test Gain.clap");
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(dir.join("Contents/MacOS")).unwrap();
    std::fs::copy(plugin_file(), dir.join("Contents/MacOS/TestGain")).unwrap();
    std::fs::write(dir.join("Contents/Info.plist"), "<plist><dict><key>CFBundleExecutable</key><string>TestGain</string></dict></plist>").unwrap();
    let libs = platform_desktop::DesktopLibraries { linked: vec![] };
    let host = ClapHost::scan_paths(&libs, &[dir.display().to_string()]);
    assert!(host.problems.is_empty(), "{:?}", host.problems);
    assert_eq!(host.effects().len(), 1);
}

mod mixer {
    //! The mixer running a CLAP effect on a clip, through the registry's
    //! `AudioPlugins`: a constant 1.0 signal through Test Gain at -6 dB.
    use super::*;
    use ve_model::*;
    use ve_ports::*;
    use ve_time::{Time, TimeRange};

    struct Ones;
    struct OnesDec;
    impl AudioDecoder for OnesDec {
        fn seek(&mut self, _: Time) -> Result<(), MediaError> {
            Ok(())
        }
        fn next_block(&mut self) -> Result<Option<AudioBlock>, MediaError> {
            Ok(Some(AudioBlock { pts: Time::ZERO, sample_rate: 48_000, channels: 2, samples: vec![1.0; 2048] }))
        }
    }
    impl MediaBackend for Ones {
        fn name(&self) -> &str {
            "ones"
        }
        fn probe(&self, _: &Resolved) -> Result<MediaInfo, MediaError> {
            unimplemented!()
        }
        fn open_video(&self, _: &Resolved) -> Result<Box<dyn VideoDecoder>, MediaError> {
            unimplemented!()
        }
        fn open_audio(&self, _: &Resolved, _: usize, _: u32, _: u16) -> Result<Box<dyn AudioDecoder>, MediaError> {
            Ok(Box::new(OnesDec))
        }
        fn open_encoder(&self, _: &Resolved, _: &EncoderSettings) -> Result<Box<dyn Encoder>, MediaError> {
            unimplemented!()
        }
    }

    #[test]
    fn the_mixer_runs_a_clap_effect_on_a_clip() {
        let host = Arc::new(host());
        let dir = std::path::PathBuf::from(env!("CARGO_TARGET_TMPDIR")).join("clap-mixer");
        std::fs::create_dir_all(&dir).unwrap();
        std::fs::write(dir.join("a.wav"), b"").unwrap();
        let mut p = Project::new("t");
        let asset = Asset { id: AssetId::new(), name: "a".into(), media: MediaRef(format!("file:{}", dir.join("a.wav").display())), info: None, variants: Vec::new(), marks: Default::default() };
        let mut params = OrdMap::new();
        params.insert("7".to_string(), Param::Constant(Value::Float(-6.0206)));
        params.insert("9".to_string(), Param::Constant(Value::Choice(0)));
        let fx = Effect { id: EffectId::new(), plugin: PluginRef { api: PluginApi::Clap, id: "org.ve.test.gain".into(), major_version: 1 }, enabled: true, params };
        let mut track = Track::new(TrackKind::Audio, "A1");
        track.clips.push_back(Arc::new(Clip {
            id: ClipId::new(),
            name: "c".into(),
            source: ClipSource::Asset { asset: asset.id, audio_stream: 0 },
            source_range: TimeRange::new(Time::ZERO, Time::from_seconds(60)),
            timeline_start: Time::ZERO,
            enabled: true,
            link: None,
            effects: [Arc::new(fx)].into_iter().collect(),
            retime: Default::default(),
            transition_in: None,
            transition_out: None,
            channels: Vec::new(),
        }));
        p.assets.insert(asset.id, Arc::new(asset));
        let seq = Sequence { id: SequenceId::new(), name: "s".into(), format: SequenceFormat::default(), tracks: [Arc::new(track)].into_iter().collect(), marks: Default::default() };
        let mut mixer = ve_media::Mixer::new(Arc::new(platform_headless::FileStorage::rooted(&dir)), Arc::new(Ones), 48_000);
        mixer.set_plugins(host);
        let mut out = vec![0f32; 1024 * 2];
        mixer.render(&p, &seq, Time::ZERO, &mut out);
        assert!(mixer.errors().next().is_none(), "{:?}", mixer.errors().collect::<Vec<_>>());
        assert!((out[500] - 0.5).abs() < 1e-3, "through the plugin at -6 dB: {}", out[500]);
    }
}
