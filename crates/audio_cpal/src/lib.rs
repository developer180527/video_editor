//! [`AudioOutput`] on cpal.
//!
//! cpal streams are not `Send` on every backend, so each stream lives on a
//! thread of its own and is driven through a channel. The callback counts
//! the frames it hands to the device; that count is the engine's master clock.

use cpal::traits::{DeviceTrait, HostTrait, StreamTrait};
use std::sync::mpsc;
use std::sync::Arc;
use ve_ports::*;

#[derive(Default)]
pub struct CpalAudio;

enum Ctl {
    Play,
    Pause,
}

pub struct CpalStream {
    ctl: mpsc::Sender<Ctl>,
    clock: Arc<AudioClock>,
    config: AudioConfig,
}

impl AudioOutput for CpalAudio {
    fn preferred(&self) -> Option<AudioConfig> {
        let device = cpal::default_host().default_output_device()?;
        let c = device.default_output_config().ok()?;
        Some(AudioConfig { sample_rate: c.sample_rate(), channels: c.channels(), buffer_frames: 512 })
    }

    fn open(&self, want: AudioConfig, mut render: RenderCallback) -> Result<Box<dyn AudioStream>, AudioError> {
        let clock = Arc::new(AudioClock::new());
        let counter = clock.clone();
        let (ctl, rx) = mpsc::channel::<Ctl>();
        let (ready_tx, ready_rx) = mpsc::channel::<Result<AudioConfig, AudioError>>();

        std::thread::Builder::new()
            .name("ve-audio".into())
            .spawn(move || {
                let open = || -> Result<(cpal::Stream, AudioConfig), AudioError> {
                    let device = cpal::default_host().default_output_device().ok_or(AudioError::NoDevice)?;
                    let config = cpal::StreamConfig {
                        channels: want.channels,
                        sample_rate: want.sample_rate,
                        buffer_size: cpal::BufferSize::Default,
                    };
                    let channels = want.channels as u64;
                    let stream = device
                        .build_output_stream(
                            config,
                            move |buf: &mut [f32], _| {
                                render(buf);
                                counter.advance(buf.len() as u64 / channels);
                            },
                            |e| eprintln!("audio stream error: {e}"),
                            None,
                        )
                        .map_err(|e| AudioError::Other(e.to_string()))?;
                    Ok((stream, want))
                };
                let stream = match open() {
                    Ok((s, cfg)) => {
                        let _ = ready_tx.send(Ok(cfg));
                        s
                    }
                    Err(e) => {
                        let _ = ready_tx.send(Err(e));
                        return;
                    }
                };
                // Runs until the `CpalStream` is dropped and the channel closes.
                for msg in rx {
                    let _ = match msg {
                        Ctl::Play => stream.play().map_err(|e| e.to_string()),
                        Ctl::Pause => stream.pause().map_err(|e| e.to_string()),
                    };
                }
            })
            .map_err(|e| AudioError::Other(e.to_string()))?;

        let config = ready_rx.recv().map_err(|_| AudioError::Other("audio thread died".into()))??;
        Ok(Box::new(CpalStream { ctl, clock, config }))
    }
}

impl AudioStream for CpalStream {
    fn play(&mut self) -> Result<(), AudioError> {
        self.ctl.send(Ctl::Play).map_err(|_| AudioError::Other("audio thread gone".into()))
    }
    fn pause(&mut self) -> Result<(), AudioError> {
        self.ctl.send(Ctl::Pause).map_err(|_| AudioError::Other("audio thread gone".into()))
    }
    fn clock(&self) -> Arc<AudioClock> {
        self.clock.clone()
    }
    fn latency_frames(&self) -> u32 {
        0 // cpal reports latency per callback; plumbed through in Phase C
    }
    fn config(&self) -> AudioConfig {
        self.config
    }
}
