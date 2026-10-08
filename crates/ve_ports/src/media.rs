use std::any::Any;
use thiserror::Error;
use ve_model::MediaInfo;
use ve_time::{Rate, Time};

use crate::Resolved;

#[derive(Debug, Error)]
pub enum MediaError {
    #[error("unsupported media: {0}")]
    Unsupported(String),
    #[error("corrupt or truncated media: {0}")]
    Corrupt(String),
    #[error("end of stream")]
    EndOfStream,
    #[error("{0}")]
    Other(String),
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum PixelFormat {
    /// 8-bit 4:2:0, two planes (Y, interleaved CbCr).
    Nv12,
    /// 10-bit 4:2:0 in 16-bit words, two planes.
    P010,
    /// 8-bit 4:2:0, three planes.
    Yuv420p,
    /// 10-bit 4:2:2 in 16-bit words, three planes (ProRes, DNxHR).
    Yuv422p10,
    Rgba8,
    /// Half-float RGBA, linear: the engine's working format.
    Rgba16f,
}

/// How the colour numbers in a frame are to be read.
#[derive(Clone, Debug, PartialEq, Eq, Default)]
pub struct ColorTags {
    /// e.g. "bt709", "bt2020"; empty when the file does not say.
    pub primaries: String,
    pub transfer: String,
    pub matrix: String,
    pub full_range: bool,
}

pub enum FrameData {
    /// Planes in system memory.
    Cpu { planes: Vec<Vec<u8>>, strides: Vec<usize> },
    /// A frame still owned by a hardware decoder (a `CVPixelBuffer`, a D3D11
    /// texture, a DMA-BUF). Only the matching GPU importer can read it, with
    /// no copy.
    Native(Box<dyn Any + Send + Sync>),
}

pub struct VideoFrame {
    /// Presentation time, in the source's timeline.
    pub pts: Time,
    pub duration: Time,
    pub width: u32,
    pub height: u32,
    pub format: PixelFormat,
    pub color: ColorTags,
    pub data: FrameData,
}

pub struct AudioBlock {
    pub pts: Time,
    pub sample_rate: u32,
    pub channels: u16,
    /// Interleaved, `frames * channels` samples.
    pub samples: Vec<f32>,
}

pub trait VideoDecoder: Send {
    /// Position so that the next frame returned is the one shown at `t`.
    fn seek(&mut self, t: Time) -> Result<(), MediaError>;
    /// The next frame in presentation order, or `None` at the end.
    fn next_frame(&mut self) -> Result<Option<VideoFrame>, MediaError>;
}

pub trait AudioDecoder: Send {
    fn seek(&mut self, t: Time) -> Result<(), MediaError>;
    /// Decoded audio converted to `sample_rate` and `channels`.
    fn next_block(&mut self) -> Result<Option<AudioBlock>, MediaError>;
}

#[derive(Clone, Debug, PartialEq)]
pub struct EncoderSettings {
    /// e.g. "h264", "hevc", "prores".
    pub video_codec: String,
    pub audio_codec: String,
    /// e.g. "mp4", "mov".
    pub container: String,
    pub width: u32,
    pub height: u32,
    pub rate: Rate,
    pub sample_rate: u32,
    pub channels: u16,
    pub video_bitrate: Option<u64>,
    pub prefer_hardware: bool,
}

pub trait Encoder: Send {
    fn push_video(&mut self, frame: VideoFrame) -> Result<(), MediaError>;
    fn push_audio(&mut self, block: AudioBlock) -> Result<(), MediaError>;
    fn finish(self: Box<Self>) -> Result<(), MediaError>;
}

/// Decoding and encoding. FFmpeg is one implementation (`media_ffmpeg`);
/// AVFoundation or Media Foundation could be others.
pub trait MediaBackend: Send + Sync {
    fn name(&self) -> &str;
    fn probe(&self, media: &Resolved) -> Result<MediaInfo, MediaError>;
    fn open_video(&self, media: &Resolved) -> Result<Box<dyn VideoDecoder>, MediaError>;
    fn open_audio(&self, media: &Resolved, sample_rate: u32, channels: u16) -> Result<Box<dyn AudioDecoder>, MediaError>;
    /// `out` must be a reference the storage port can write.
    fn open_encoder(&self, out: &Resolved, settings: &EncoderSettings) -> Result<Box<dyn Encoder>, MediaError>;
}
