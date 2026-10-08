//! Encoding: H.264 / HEVC through VideoToolbox, ProRes 422 HQ through
//! FFmpeg's own (LGPL) encoder, AAC audio, into MP4 or MOV.
//!
//! Video arrives as display-encoded RGBA8 (what the compositor's display
//! pass produces) and leaves as Rec.709 YCbCr, tagged as such.

use std::ffi::{c_int, CString};
use std::ptr;

use ve_ports::*;

use crate::{err, sys};

const EAGAIN: c_int = -(sys::EAGAIN as c_int);
const EOF: c_int = -((b'E' as c_int) | (b'O' as c_int) << 8 | (b'F' as c_int) << 16 | (b' ' as c_int) << 24);

struct Stream {
    ctx: *mut sys::AVCodecContext,
    st: *mut sys::AVStream,
}

pub struct FfEncoder {
    oc: *mut sys::AVFormatContext,
    video: Stream,
    audio: Option<Stream>,
    sws: *mut sys::SwsContext,
    pkt: *mut sys::AVPacket,
    frames: i64,
    samples: i64,
    /// Interleaved stereo waiting for a full AAC frame.
    pending: Vec<f32>,
    finished: bool,
}

unsafe impl Send for FfEncoder {}

fn check(r: c_int, what: &str) -> Result<c_int, MediaError> {
    if r < 0 {
        Err(MediaError::Other(format!("{what}: {}", err(r))))
    } else {
        Ok(r)
    }
}

impl FfEncoder {
    pub fn open(out: &Resolved, s: &EncoderSettings) -> Result<FfEncoder, MediaError> {
        let path = out.path.as_ref().ok_or_else(|| MediaError::Unsupported("export needs a file path".into()))?;
        let cpath = CString::new(path.to_string_lossy().as_bytes()).unwrap();
        let container = CString::new(s.container.as_str()).unwrap();
        unsafe {
            let mut oc = ptr::null_mut();
            check(sys::avformat_alloc_output_context2(&mut oc, ptr::null(), container.as_ptr(), cpath.as_ptr()), "output format")?;
            let mut enc = FfEncoder {
                oc,
                video: Stream { ctx: ptr::null_mut(), st: ptr::null_mut() },
                audio: None,
                sws: sys::sws_alloc_context(),
                pkt: sys::av_packet_alloc(),
                frames: 0,
                samples: 0,
                pending: Vec::new(),
                finished: false,
            };
            enc.video = enc.add_video(s)?;
            if !s.audio_codec.is_empty() {
                enc.audio = Some(enc.add_audio(s)?);
            }
            check(sys::avio_open(&mut (*oc).pb, cpath.as_ptr(), sys::AVIO_FLAG_WRITE as c_int), "open output")?;
            check(sys::avformat_write_header(oc, ptr::null_mut()), "write header")?;
            Ok(enc)
        }
    }

    unsafe fn add_video(&mut self, s: &EncoderSettings) -> Result<Stream, MediaError> {
        let (name, pix) = match s.video_codec.as_str() {
            "hevc" => ("hevc_videotoolbox", sys::AVPixelFormat::AV_PIX_FMT_NV12),
            "prores" => ("prores_ks", sys::AVPixelFormat::AV_PIX_FMT_YUV422P10LE),
            _ => ("h264_videotoolbox", sys::AVPixelFormat::AV_PIX_FMT_NV12),
        };
        let cname = CString::new(name).unwrap();
        let codec = sys::avcodec_find_encoder_by_name(cname.as_ptr());
        if codec.is_null() {
            return Err(MediaError::Unsupported(format!("encoder {name} is not in this build")));
        }
        let st = sys::avformat_new_stream(self.oc, ptr::null());
        let ctx = sys::avcodec_alloc_context3(codec);
        (*ctx).width = s.width as c_int;
        (*ctx).height = s.height as c_int;
        (*ctx).time_base = sys::AVRational { num: s.rate.den as c_int, den: s.rate.num as c_int };
        (*ctx).framerate = sys::AVRational { num: s.rate.num as c_int, den: s.rate.den as c_int };
        (*ctx).pix_fmt = pix;
        (*ctx).color_primaries = sys::AVColorPrimaries::AVCOL_PRI_BT709;
        (*ctx).color_trc = sys::AVColorTransferCharacteristic::AVCOL_TRC_BT709;
        (*ctx).colorspace = sys::AVColorSpace::AVCOL_SPC_BT709;
        (*ctx).color_range = sys::AVColorRange::AVCOL_RANGE_MPEG;
        let pixels = (s.width * s.height) as u64;
        (*ctx).bit_rate = s.video_bitrate.unwrap_or(match name {
            "hevc_videotoolbox" => pixels * 6,  // ~12 Mb/s at 1080p
            _ => pixels * 10,                    // ~20 Mb/s at 1080p
        }) as i64;
        if name == "prores_ks" {
            let (k, v) = (CString::new("profile").unwrap(), CString::new("hq").unwrap());
            sys::av_opt_set((*ctx).priv_data, k.as_ptr(), v.as_ptr(), 0);
        }
        if (*(*self.oc).oformat).flags & sys::AVFMT_GLOBALHEADER as c_int != 0 {
            (*ctx).flags |= sys::AV_CODEC_FLAG_GLOBAL_HEADER as c_int;
        }
        check(sys::avcodec_open2(ctx, codec, ptr::null_mut()), name)?;
        check(sys::avcodec_parameters_from_context((*st).codecpar, ctx), "video parameters")?;
        (*st).time_base = (*ctx).time_base;
        (*st).avg_frame_rate = (*ctx).framerate;
        Ok(Stream { ctx, st })
    }

    unsafe fn add_audio(&mut self, s: &EncoderSettings) -> Result<Stream, MediaError> {
        let cname = CString::new("aac").unwrap();
        let codec = sys::avcodec_find_encoder_by_name(cname.as_ptr());
        if codec.is_null() {
            return Err(MediaError::Unsupported("AAC encoder missing".into()));
        }
        let st = sys::avformat_new_stream(self.oc, ptr::null());
        let ctx = sys::avcodec_alloc_context3(codec);
        (*ctx).sample_fmt = sys::AVSampleFormat::AV_SAMPLE_FMT_FLTP;
        (*ctx).sample_rate = s.sample_rate as c_int;
        sys::av_channel_layout_default(&mut (*ctx).ch_layout, s.channels as c_int);
        (*ctx).bit_rate = 192_000 * s.channels as i64 / 2;
        (*ctx).time_base = sys::AVRational { num: 1, den: s.sample_rate as c_int };
        if (*(*self.oc).oformat).flags & sys::AVFMT_GLOBALHEADER as c_int != 0 {
            (*ctx).flags |= sys::AV_CODEC_FLAG_GLOBAL_HEADER as c_int;
        }
        check(sys::avcodec_open2(ctx, codec, ptr::null_mut()), "aac")?;
        check(sys::avcodec_parameters_from_context((*st).codecpar, ctx), "audio parameters")?;
        (*st).time_base = (*ctx).time_base;
        Ok(Stream { ctx, st })
    }

    /// Send a frame (or `null` to flush) and write whatever comes out.
    unsafe fn send(&mut self, audio: bool, frame: *const sys::AVFrame) -> Result<(), MediaError> {
        let s = if audio { self.audio.as_ref().unwrap() } else { &self.video };
        let (ctx, st) = (s.ctx, s.st);
        let r = sys::avcodec_send_frame(ctx, frame);
        if r < 0 && r != EOF {
            return Err(MediaError::Other(format!("encode: {}", err(r))));
        }
        loop {
            let r = sys::avcodec_receive_packet(ctx, self.pkt);
            if r == EAGAIN || r == EOF {
                return Ok(());
            }
            check(r, "encode")?;
            // Some hardware encoders leave durations unset; a video frame
            // lasts one tick of the codec's frame-rate time base.
            if !audio && (*self.pkt).duration == 0 {
                (*self.pkt).duration = 1;
            }
            sys::av_packet_rescale_ts(self.pkt, (*ctx).time_base, (*st).time_base);
            (*self.pkt).stream_index = (*st).index;
            check(sys::av_interleaved_write_frame(self.oc, self.pkt), "write")?;
        }
    }

    unsafe fn send_audio_frame(&mut self, samples: &[f32], n: usize) -> Result<(), MediaError> {
        let ctx = self.audio.as_ref().unwrap().ctx;
        let mut f = sys::av_frame_alloc();
        (*f).nb_samples = n as c_int;
        (*f).format = sys::AVSampleFormat::AV_SAMPLE_FMT_FLTP as c_int;
        (*f).sample_rate = (*ctx).sample_rate;
        sys::av_channel_layout_copy(&mut (*f).ch_layout, &(*ctx).ch_layout);
        check(sys::av_frame_get_buffer(f, 0), "audio frame")?;
        let ch = (*ctx).ch_layout.nb_channels as usize;
        for c in 0..ch {
            let plane = std::slice::from_raw_parts_mut((*f).data[c] as *mut f32, n);
            for (i, s) in plane.iter_mut().enumerate() {
                *s = samples.get(i * ch + c).copied().unwrap_or(0.0);
            }
        }
        (*f).pts = self.samples;
        self.samples += n as i64;
        let r = self.send(true, f);
        sys::av_frame_free(&mut f);
        r
    }
}

impl Encoder for FfEncoder {
    fn push_video(&mut self, frame: VideoFrame) -> Result<(), MediaError> {
        let FrameData::Cpu { planes, strides } = &frame.data else {
            return Err(MediaError::Unsupported("export frames must be in memory".into()));
        };
        if frame.format != PixelFormat::Rgba8 {
            return Err(MediaError::Unsupported("export frames must be RGBA8".into()));
        }
        unsafe {
            let ctx = self.video.ctx;
            let mut src = sys::av_frame_alloc();
            (*src).width = frame.width as c_int;
            (*src).height = frame.height as c_int;
            (*src).format = sys::AVPixelFormat::AV_PIX_FMT_RGBA as c_int;
            (*src).data[0] = planes[0].as_ptr() as *mut u8;
            (*src).linesize[0] = strides[0] as c_int;
            (*src).colorspace = sys::AVColorSpace::AVCOL_SPC_RGB;
            (*src).color_range = sys::AVColorRange::AVCOL_RANGE_JPEG;
            let mut dst = sys::av_frame_alloc();
            (*dst).width = (*ctx).width;
            (*dst).height = (*ctx).height;
            (*dst).format = (*ctx).pix_fmt as c_int;
            (*dst).colorspace = sys::AVColorSpace::AVCOL_SPC_BT709;
            (*dst).color_primaries = sys::AVColorPrimaries::AVCOL_PRI_BT709;
            (*dst).color_trc = sys::AVColorTransferCharacteristic::AVCOL_TRC_BT709;
            (*dst).color_range = sys::AVColorRange::AVCOL_RANGE_MPEG;
            let r = sys::sws_scale_frame(self.sws, dst, src);
            sys::av_frame_free(&mut src);
            if r < 0 {
                sys::av_frame_free(&mut dst);
                return Err(MediaError::Other(format!("colour conversion: {}", err(r))));
            }
            (*dst).pts = self.frames;
            self.frames += 1;
            let r = self.send(false, dst);
            sys::av_frame_free(&mut dst);
            r
        }
    }

    fn push_audio(&mut self, block: AudioBlock) -> Result<(), MediaError> {
        let Some(a) = &self.audio else { return Ok(()) };
        let size = unsafe { (*a.ctx).frame_size.max(1024) } as usize;
        self.pending.extend_from_slice(&block.samples);
        let ch = block.channels.max(1) as usize;
        while self.pending.len() >= size * ch {
            let chunk: Vec<f32> = self.pending.drain(..size * ch).collect();
            unsafe { self.send_audio_frame(&chunk, size)? };
        }
        Ok(())
    }

    fn finish(mut self: Box<Self>) -> Result<(), MediaError> {
        unsafe {
            if let Some(a) = &self.audio {
                let size = (*a.ctx).frame_size.max(1024) as usize;
                if !self.pending.is_empty() {
                    let rest = std::mem::take(&mut self.pending);
                    self.send_audio_frame(&rest, size)?; // padded with silence
                }
                self.send(true, ptr::null())?;
            }
            self.send(false, ptr::null())?;
            check(sys::av_write_trailer(self.oc), "write trailer")?;
            self.finished = true;
        }
        Ok(())
    }
}

impl Drop for FfEncoder {
    fn drop(&mut self) {
        unsafe {
            for s in [Some(&mut self.video), self.audio.as_mut()].into_iter().flatten() {
                sys::avcodec_free_context(&mut s.ctx);
            }
            if !self.oc.is_null() {
                if !(*self.oc).pb.is_null() {
                    sys::avio_closep(&mut (*self.oc).pb);
                }
                sys::avformat_free_context(self.oc);
            }
            sys::sws_free_context(&mut self.sws);
            sys::av_packet_free(&mut self.pkt);
        }
    }
}
