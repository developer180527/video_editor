//! [`MediaBackend`] on FFmpeg.
//!
//! Probing, decoding (VideoToolbox where available) and encoding.

mod decode;
mod encode;
mod sys;

pub use encode::FfEncoder;

pub use decode::{AudioDec, VideoDec};

use std::ffi::{CStr, CString};
use std::ptr;
use ve_model::{AudioStreamInfo, MediaInfo, VideoStreamInfo};
use ve_ports::*;
use ve_time::{Rate, Time, TICKS_PER_SECOND};

pub struct Ffmpeg;

impl Ffmpeg {
    pub fn new() -> Self {
        // Errors reach the caller as `MediaError`s; FFmpeg's own chatter (a
        // hardware decoder declining a codec, say) is noise on stderr.
        unsafe { sys::av_log_set_level(sys::AV_LOG_FATAL as i32) };
        Ffmpeg
    }

    /// e.g. "9.0.2".
    pub fn version() -> String {
        unsafe { CStr::from_ptr(sys::av_version_info()) }.to_string_lossy().into_owned()
    }
}

impl Default for Ffmpeg {
    fn default() -> Self {
        Self::new()
    }
}

fn err(code: i32) -> String {
    let mut buf = [0 as std::ffi::c_char; 128];
    unsafe { sys::av_strerror(code, buf.as_mut_ptr(), buf.len()) };
    unsafe { CStr::from_ptr(buf.as_ptr()) }.to_string_lossy().into_owned()
}

/// An open demuxer, closed on drop.
struct Input(*mut sys::AVFormatContext);

impl Input {
    fn open(media: &Resolved) -> Result<Input, MediaError> {
        let path = media
            .path
            .as_ref()
            .ok_or_else(|| MediaError::Unsupported("stream-only media (custom AVIO comes in Phase C)".into()))?;
        let c = CString::new(path.to_string_lossy().as_bytes()).map_err(|e| MediaError::Other(e.to_string()))?;
        let mut ctx = ptr::null_mut();
        let r = unsafe { sys::avformat_open_input(&mut ctx, c.as_ptr(), ptr::null(), ptr::null_mut()) };
        if r < 0 {
            return Err(MediaError::Unsupported(format!("{}: {}", path.display(), err(r))));
        }
        let input = Input(ctx);
        let r = unsafe { sys::avformat_find_stream_info(ctx, ptr::null_mut()) };
        if r < 0 {
            return Err(MediaError::Corrupt(err(r)));
        }
        Ok(input)
    }

    /// Where the file's timeline starts. Stream timestamps are measured from
    /// here, so source time 0 is the first picture or sound in the file
    /// (an MPEG-TS often starts at 1.4 s; a camera file at its timecode),
    /// and audio and video keep their offset to each other.
    fn origin(&self) -> ve_time::Time {
        let start = unsafe { (*self.0).start_time };
        if start == i64::MIN {
            ve_time::Time::ZERO // AV_NOPTS_VALUE
        } else {
            to_time(start, sys::AVRational { num: 1, den: sys::AV_TIME_BASE as i32 })
        }
    }

    fn streams(&self) -> &[*mut sys::AVStream] {
        unsafe { std::slice::from_raw_parts((*self.0).streams, (*self.0).nb_streams as usize) }
    }
}

impl Drop for Input {
    fn drop(&mut self) {
        unsafe { sys::avformat_close_input(&mut self.0) };
    }
}

fn codec_name(id: sys::AVCodecID) -> String {
    unsafe { CStr::from_ptr(sys::avcodec_get_name(id)) }.to_string_lossy().into_owned()
}

/// `value` in `tb` units as exact ticks (rounded only if `tb` is unusual).
fn to_time(value: i64, tb: sys::AVRational) -> Time {
    let x = value as i128 * tb.num as i128 * TICKS_PER_SECOND as i128;
    Time((x / tb.den as i128) as i64)
}

impl MediaBackend for Ffmpeg {
    fn name(&self) -> &str {
        "FFmpeg"
    }

    fn probe(&self, media: &Resolved) -> Result<MediaInfo, MediaError> {
        let input = Input::open(media)?;
        let mut info = MediaInfo { duration: Time::ZERO, video: None, audio: Vec::new() };
        let ctx = unsafe { &*input.0 };
        if ctx.duration > 0 {
            info.duration = to_time(ctx.duration, sys::AVRational { num: 1, den: sys::AV_TIME_BASE as i32 });
        }
        for &s in input.streams() {
            let s = unsafe { &*s };
            let par = unsafe { &*s.codecpar };
            match par.codec_type {
                sys::AVMediaType::AVMEDIA_TYPE_VIDEO if info.video.is_none() => {
                    let r = if s.avg_frame_rate.num > 0 { s.avg_frame_rate } else { s.r_frame_rate };
                    info.video = Some(VideoStreamInfo {
                        width: par.width as u32,
                        height: par.height as u32,
                        rate: if r.num > 0 && r.den > 0 { Rate::new(r.num as u32, r.den as u32) } else { Rate::FPS_24 },
                        codec: codec_name(par.codec_id),
                    });
                }
                sys::AVMediaType::AVMEDIA_TYPE_AUDIO => {
                    let channels = par.ch_layout.nb_channels;
                    // Undeclared layouts (common in camera files) play as the
                    // usual layout for their count; name them so.
                    let layout = match (par.ch_layout.order, channels) {
                        (sys::AVChannelOrder::AV_CHANNEL_ORDER_UNSPEC, 1) => "mono".to_string(),
                        (sys::AVChannelOrder::AV_CHANNEL_ORDER_UNSPEC, 2) => "stereo".to_string(),
                        (sys::AVChannelOrder::AV_CHANNEL_ORDER_UNSPEC, n) => format!("{n} channels"),
                        _ => {
                            let mut name = [0 as std::ffi::c_char; 64];
                            let n = unsafe { sys::av_channel_layout_describe(&par.ch_layout, name.as_mut_ptr(), name.len()) };
                            if n > 0 { unsafe { CStr::from_ptr(name.as_ptr()) }.to_string_lossy().into_owned() } else { format!("{channels} channels") }
                        }
                    };
                    info.audio.push(AudioStreamInfo {
                        sample_rate: par.sample_rate as u32,
                        channels: par.ch_layout.nb_channels as u16,
                        codec: codec_name(par.codec_id),
                        layout,
                    });
                }
                _ => {}
            }
        }
        Ok(info)
    }

    fn open_video(&self, media: &Resolved) -> Result<Box<dyn VideoDecoder>, MediaError> {
        Ok(Box::new(VideoDec::open(media, true)?))
    }

    fn open_video_for_gpu(&self, media: &Resolved) -> Result<Box<dyn VideoDecoder>, MediaError> {
        Ok(Box::new(VideoDec::open_with(media, true, true)?))
    }

    fn open_audio(&self, media: &Resolved, stream: usize, sample_rate: u32, channels: u16) -> Result<Box<dyn AudioDecoder>, MediaError> {
        Ok(Box::new(AudioDec::open(media, stream, sample_rate, channels)?))
    }

    fn open_encoder(&self, out: &Resolved, settings: &EncoderSettings) -> Result<Box<dyn Encoder>, MediaError> {
        Ok(Box::new(FfEncoder::open(out, settings)?))
    }
}

/// Hardware device types in this FFmpeg build.
pub(crate) fn built_hw_types() -> Vec<sys::AVHWDeviceType> {
    let mut v = Vec::new();
    let mut t = sys::AVHWDeviceType::AV_HWDEVICE_TYPE_NONE;
    loop {
        t = unsafe { sys::av_hwdevice_iterate_types(t) };
        if t == sys::AVHWDeviceType::AV_HWDEVICE_TYPE_NONE {
            return v;
        }
        v.push(t);
    }
}

/// One device per type for the whole process, shared by every decoder
/// (creating one — a CUDA context, say — can take a tenth of a second).
/// `None` once a type has failed to open, so it is not retried per clip.
pub(crate) fn shared_device(t: sys::AVHWDeviceType) -> Option<*mut sys::AVBufferRef> {
    use std::collections::HashMap;
    use std::sync::{Mutex, OnceLock};
    // Pointers as integers: the device contexts are thread-safe and live
    // for the whole process.
    static DEVICES: OnceLock<Mutex<HashMap<i32, Option<usize>>>> = OnceLock::new();
    let mut devices = DEVICES.get_or_init(Default::default).lock().unwrap();
    let dev = *devices.entry(t as i32).or_insert_with(|| {
        let mut dev = ptr::null_mut();
        let r = unsafe { sys::av_hwdevice_ctx_create(&mut dev, t, ptr::null(), ptr::null_mut(), 0) };
        (r >= 0).then_some(dev as usize)
    });
    // A new reference for the caller.
    dev.map(|d| unsafe { sys::av_buffer_ref(d as *mut sys::AVBufferRef) }).filter(|r| !r.is_null())
}
