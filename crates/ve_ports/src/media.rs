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
    /// 16 bits per channel, little-endian: the deep output export encodes.
    Rgba16,
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

/// Planes in system memory that belong to someone else — a decoder's
/// buffer pool — and stay valid while this lives. Lets a decoder hand out a
/// frame without copying it.
pub trait SharedPlanes: Send + Sync {
    fn planes(&self) -> Vec<&[u8]>;
    fn strides(&self) -> Vec<usize>;
}

/// A frame's planes in system memory, however they are held: borrowed from
/// the frame, or copied out of GPU memory for a consumer that needs bytes.
pub struct CpuPlanes<'a> {
    pub planes: Vec<std::borrow::Cow<'a, [u8]>>,
    pub strides: Vec<usize>,
}

/// What a frame left in GPU memory is, for the importer that matches it.
/// Each platform's GPU interop reads its own kind; the rest is reserved for
/// the interop that discrete GPUs on Windows and Linux will use.
#[derive(Clone, Copy, Debug)]
#[non_exhaustive]
pub enum NativeHandle {
    /// A `CVPixelBufferRef` (Apple: VideoToolbox output, IOSurface-backed).
    CvPixelBuffer(*mut std::ffi::c_void),
}

/// A decoded frame still in the decoder's GPU memory.
pub trait NativeFrame: Send + Sync {
    /// Valid while this frame lives.
    fn handle(&self) -> NativeHandle;
    /// The same picture as planes in memory (a copy), for consumers that
    /// cannot import the handle. Same layout as the frame's `format`.
    fn to_cpu(&self) -> Option<(Vec<Vec<u8>>, Vec<usize>)>;
}

pub enum FrameData {
    /// Planes in system memory, owned.
    Cpu { planes: Vec<Vec<u8>>, strides: Vec<usize> },
    /// Planes in system memory, shared with the decoder (no copy).
    Shared(Box<dyn SharedPlanes>),
    /// A frame still owned by a hardware decoder, in GPU memory. The
    /// matching importer reads it with no copy; anything else gets a copy
    /// through [`FrameData::cpu`].
    Native(Box<dyn NativeFrame>),
}

impl FrameData {
    /// The planes, when they are in system memory (owned or shared).
    pub fn cpu(&self) -> Option<CpuPlanes<'_>> {
        match self {
            FrameData::Cpu { planes, strides } => Some(CpuPlanes { planes: planes.iter().map(|p| p.as_slice().into()).collect(), strides: strides.clone() }),
            FrameData::Shared(s) => Some(CpuPlanes { planes: s.planes().into_iter().map(Into::into).collect(), strides: s.strides() }),
            FrameData::Native(n) => n.to_cpu().map(|(planes, strides)| CpuPlanes { planes: planes.into_iter().map(Into::into).collect(), strides }),
        }
    }
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
    /// Like `open_video`, but frames may stay in GPU memory
    /// ([`FrameData::Native`]) when the hardware decodes them: for a consumer
    /// with a matching importer.
    fn open_video_for_gpu(&self, media: &Resolved) -> Result<Box<dyn VideoDecoder>, MediaError> {
        self.open_video(media)
    }
    /// The file's `stream`-th audio stream (an index into
    /// `MediaInfo::audio`), converted to `sample_rate` and `channels` by its
    /// channel layout (mono is centred, 5.1 is downmixed, …).
    fn open_audio(&self, media: &Resolved, stream: usize, sample_rate: u32, channels: u16) -> Result<Box<dyn AudioDecoder>, MediaError>;
    /// `out` must be a reference the storage port can write.
    fn open_encoder(&self, out: &Resolved, settings: &EncoderSettings) -> Result<Box<dyn Encoder>, MediaError>;
}
