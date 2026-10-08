//! Video and audio decoders.
//!
//! Video decodes with VideoToolbox where the platform has it (falling back to
//! software), and is handed out as one of two formats so the compositor needs
//! only two YUV paths: NV12 for 8-bit sources, P010 for deeper ones. Audio is
//! resampled to interleaved `f32` at the rate and channel count asked for.

use std::ffi::{c_int, CStr};
use std::ptr;

use ve_ports::*;
use ve_time::{Rate, Time, TICKS_PER_SECOND};

use crate::{err, sys, to_time, Input};

/// `AV_NOPTS_VALUE` (a cast macro bindgen cannot evaluate).
const NOPTS: i64 = i64::MIN;
/// `AVERROR(EAGAIN)`.
const EAGAIN: c_int = -(sys::EAGAIN as c_int);
/// `AVERROR_EOF`: FFERRTAG('E','O','F',' ').
const EOF: c_int = -((b'E' as c_int) | (b'O' as c_int) << 8 | (b'F' as c_int) << 16 | (b' ' as c_int) << 24);

fn to_ts(t: Time, tb: sys::AVRational) -> i64 {
    (t.ticks() as i128 * tb.den as i128 / (tb.num as i128 * TICKS_PER_SECOND as i128)) as i64
}

fn name(p: *const std::ffi::c_char) -> String {
    if p.is_null() {
        String::new()
    } else {
        unsafe { CStr::from_ptr(p) }.to_string_lossy().into_owned()
    }
}

/// The codec context and the packet/frame scratch every decoder needs.
struct Codec {
    ctx: *mut sys::AVCodecContext,
    pkt: *mut sys::AVPacket,
    frame: *mut sys::AVFrame,
    eof_sent: bool,
}

impl Codec {
    fn open(input: &Input, stream: usize, hw: bool) -> Result<(Codec, bool), MediaError> {
        unsafe {
            let st = input.streams()[stream];
            let par = (*st).codecpar;
            let codec = sys::avcodec_find_decoder((*par).codec_id);
            if codec.is_null() {
                return Err(MediaError::Unsupported(format!("no decoder for {}", crate::codec_name((*par).codec_id))));
            }
            let ctx = sys::avcodec_alloc_context3(codec);
            sys::avcodec_parameters_to_context(ctx, par);
            (*ctx).pkt_timebase = (*st).time_base;
            (*ctx).thread_count = 0; // automatic
            let mut using_hw = false;
            if hw && (*par).codec_type == sys::AVMediaType::AVMEDIA_TYPE_VIDEO {
                let mut dev = ptr::null_mut();
                let r = sys::av_hwdevice_ctx_create(
                    &mut dev,
                    sys::AVHWDeviceType::AV_HWDEVICE_TYPE_VIDEOTOOLBOX,
                    ptr::null(),
                    ptr::null_mut(),
                    0,
                );
                if r >= 0 {
                    (*ctx).hw_device_ctx = dev;
                    (*ctx).get_format = Some(pick_hw_format);
                    using_hw = true;
                }
            }
            let r = sys::avcodec_open2(ctx, codec, ptr::null_mut());
            if r < 0 {
                let mut c = ctx;
                sys::avcodec_free_context(&mut c);
                return Err(MediaError::Unsupported(err(r)));
            }
            Ok((Codec { ctx, pkt: sys::av_packet_alloc(), frame: sys::av_frame_alloc(), eof_sent: false }, using_hw))
        }
    }

    /// The next decoded frame of `stream` into `self.frame`; false at the end.
    fn next(&mut self, input: &Input, stream: usize) -> Result<bool, MediaError> {
        unsafe {
            loop {
                let r = sys::avcodec_receive_frame(self.ctx, self.frame);
                if r == 0 {
                    return Ok(true);
                }
                if r == EOF {
                    return Ok(false);
                }
                if r != EAGAIN {
                    return Err(MediaError::Corrupt(err(r)));
                }
                if self.eof_sent {
                    return Ok(false);
                }
                let r = sys::av_read_frame(input.0, self.pkt);
                if r < 0 {
                    sys::avcodec_send_packet(self.ctx, ptr::null());
                    self.eof_sent = true;
                    continue;
                }
                if (*self.pkt).stream_index as usize == stream {
                    let r = sys::avcodec_send_packet(self.ctx, self.pkt);
                    if r < 0 && r != EAGAIN {
                        sys::av_packet_unref(self.pkt);
                        return Err(MediaError::Corrupt(err(r)));
                    }
                }
                sys::av_packet_unref(self.pkt);
            }
        }
    }

    fn seek(&mut self, input: &Input, stream: usize, t: Time) -> Result<(), MediaError> {
        unsafe {
            let tb = (*input.streams()[stream]).time_base;
            let r = sys::av_seek_frame(input.0, stream as c_int, to_ts(t, tb), sys::AVSEEK_FLAG_BACKWARD as c_int);
            if r < 0 {
                return Err(MediaError::Other(format!("seek: {}", err(r))));
            }
            sys::avcodec_flush_buffers(self.ctx);
            self.eof_sent = false;
            Ok(())
        }
    }
}

impl Drop for Codec {
    fn drop(&mut self) {
        unsafe {
            sys::av_frame_free(&mut self.frame);
            sys::av_packet_free(&mut self.pkt);
            sys::avcodec_free_context(&mut self.ctx);
        }
    }
}

unsafe extern "C" fn pick_hw_format(_ctx: *mut sys::AVCodecContext, fmts: *const sys::AVPixelFormat) -> sys::AVPixelFormat {
    let mut p = fmts;
    let first = *fmts;
    while *p != sys::AVPixelFormat::AV_PIX_FMT_NONE {
        if *p == sys::AVPixelFormat::AV_PIX_FMT_VIDEOTOOLBOX {
            return *p;
        }
        p = p.add(1);
    }
    first // the decoder's own choice: software
}

fn first_stream(input: &Input, kind: sys::AVMediaType) -> Option<usize> {
    input.streams().iter().position(|&s| unsafe { (*(*s).codecpar).codec_type == kind })
}

pub struct VideoDec {
    codec: Codec,
    input: Input,
    stream: usize,
    tb: sys::AVRational,
    frame_dur: Time,
    sw: *mut sys::AVFrame,
    sws: *mut sys::SwsContext,
    skip_until: Option<Time>,
    pub hardware: bool,
}

// The FFmpeg objects are owned exclusively by this decoder.
unsafe impl Send for VideoDec {}

impl VideoDec {
    pub fn open(media: &Resolved, hw: bool) -> Result<VideoDec, MediaError> {
        let input = Input::open(media)?;
        let stream = first_stream(&input, sys::AVMediaType::AVMEDIA_TYPE_VIDEO).ok_or_else(|| MediaError::Unsupported("no video stream".into()))?;
        let (codec, hardware) = match Codec::open(&input, stream, hw) {
            Ok(c) => c,
            Err(_) if hw => Codec::open(&input, stream, false)?,
            Err(e) => return Err(e),
        };
        let st = input.streams()[stream];
        let (tb, r) = unsafe { ((*st).time_base, if (*st).avg_frame_rate.num > 0 { (*st).avg_frame_rate } else { (*st).r_frame_rate }) };
        let rate = if r.num > 0 && r.den > 0 { Rate::new(r.num as u32, r.den as u32) } else { Rate::FPS_24 };
        Ok(VideoDec {
            codec,
            input,
            stream,
            tb,
            frame_dur: rate.frame_duration(),
            sw: unsafe { sys::av_frame_alloc() },
            sws: ptr::null_mut(),
            skip_until: None,
            hardware,
        })
    }

    /// The decoded frame as NV12 or P010, copied out of FFmpeg.
    fn convert(&mut self) -> Result<VideoFrame, MediaError> {
        unsafe {
            let mut src = self.codec.frame;
            if (*src).format == sys::AVPixelFormat::AV_PIX_FMT_VIDEOTOOLBOX as c_int {
                sys::av_frame_unref(self.sw);
                let r = sys::av_hwframe_transfer_data(self.sw, src, 0);
                if r < 0 {
                    return Err(MediaError::Other(format!("hardware frame transfer: {}", err(r))));
                }
                src = self.sw;
            }
            let fmt: sys::AVPixelFormat = std::mem::transmute((*src).format);
            let desc = sys::av_pix_fmt_desc_get(fmt);
            let deep = !desc.is_null() && (*desc).comp[0].depth > 8;
            let (want, out_fmt) = if deep {
                (sys::AVPixelFormat::AV_PIX_FMT_P010LE, PixelFormat::P010)
            } else {
                (sys::AVPixelFormat::AV_PIX_FMT_NV12, PixelFormat::Nv12)
            };
            let mut dst = ptr::null_mut();
            let ready = if fmt == want {
                src
            } else {
                if self.sws.is_null() {
                    self.sws = sys::sws_alloc_context();
                }
                dst = sys::av_frame_alloc();
                (*dst).width = (*src).width;
                (*dst).height = (*src).height;
                (*dst).format = want as c_int;
                let r = sys::sws_scale_frame(self.sws, dst, src);
                if r < 0 {
                    sys::av_frame_free(&mut dst);
                    return Err(MediaError::Other(format!("pixel conversion: {}", err(r))));
                }
                dst
            };
            let (w, h) = ((*ready).width as u32, (*ready).height as u32);
            let rows = [h as usize, h.div_ceil(2) as usize];
            let mut planes = Vec::with_capacity(2);
            let mut strides = Vec::with_capacity(2);
            for (i, &n) in rows.iter().enumerate() {
                let stride = (*ready).linesize[i] as usize;
                planes.push(std::slice::from_raw_parts((*ready).data[i], stride * n).to_vec());
                strides.push(stride);
            }
            let f = self.codec.frame;
            let pts = if (*f).best_effort_timestamp != NOPTS { (*f).best_effort_timestamp } else { (*f).pts };
            let dur = if (*f).duration > 0 { to_time((*f).duration, self.tb) } else { self.frame_dur };
            let color = ColorTags {
                primaries: name(sys::av_color_primaries_name((*f).color_primaries)),
                transfer: name(sys::av_color_transfer_name((*f).color_trc)),
                matrix: name(sys::av_color_space_name((*f).colorspace)),
                full_range: (*f).color_range == sys::AVColorRange::AVCOL_RANGE_JPEG,
            };
            if !dst.is_null() {
                sys::av_frame_free(&mut dst);
            }
            Ok(VideoFrame {
                pts: to_time(pts, self.tb),
                duration: dur,
                width: w,
                height: h,
                format: out_fmt,
                color,
                data: FrameData::Cpu { planes, strides },
            })
        }
    }
}

impl Drop for VideoDec {
    fn drop(&mut self) {
        unsafe {
            sys::av_frame_free(&mut self.sw);
            if !self.sws.is_null() {
                sys::sws_free_context(&mut self.sws);
            }
        }
    }
}

impl VideoDecoder for VideoDec {
    fn seek(&mut self, t: Time) -> Result<(), MediaError> {
        self.codec.seek(&self.input, self.stream, t)?;
        self.skip_until = Some(t);
        Ok(())
    }

    fn next_frame(&mut self) -> Result<Option<VideoFrame>, MediaError> {
        loop {
            if !self.codec.next(&self.input, self.stream)? {
                return Ok(None);
            }
            let f = self.codec.frame;
            let pts = unsafe { if (*f).best_effort_timestamp != NOPTS { (*f).best_effort_timestamp } else { (*f).pts } };
            let start = to_time(pts, self.tb);
            if let Some(target) = self.skip_until {
                // Frames that end before the target are decoded but not returned.
                if start + self.frame_dur <= target {
                    unsafe { sys::av_frame_unref(f) };
                    continue;
                }
                self.skip_until = None;
            }
            let out = self.convert();
            unsafe { sys::av_frame_unref(f) };
            return out.map(Some);
        }
    }
}

pub struct AudioDec {
    codec: Codec,
    input: Input,
    stream: usize,
    tb: sys::AVRational,
    swr: *mut sys::SwrContext,
    rate: u32,
    channels: u16,
    skip_until: Option<Time>,
}

unsafe impl Send for AudioDec {}

impl AudioDec {
    pub fn open(media: &Resolved, rate: u32, channels: u16) -> Result<AudioDec, MediaError> {
        let input = Input::open(media)?;
        let stream = first_stream(&input, sys::AVMediaType::AVMEDIA_TYPE_AUDIO).ok_or_else(|| MediaError::Unsupported("no audio stream".into()))?;
        let (codec, _) = Codec::open(&input, stream, false)?;
        let tb = unsafe { (*input.streams()[stream]).time_base };
        Ok(AudioDec { codec, input, stream, tb, swr: ptr::null_mut(), rate, channels, skip_until: None })
    }

    fn resampler(&mut self) -> Result<(), MediaError> {
        if !self.swr.is_null() {
            return Ok(());
        }
        unsafe {
            let f = self.codec.frame;
            let mut out_layout: sys::AVChannelLayout = std::mem::zeroed();
            sys::av_channel_layout_default(&mut out_layout, self.channels as c_int);
            let in_fmt: sys::AVSampleFormat = std::mem::transmute((*f).format);
            let r = sys::swr_alloc_set_opts2(
                &mut self.swr,
                &out_layout,
                sys::AVSampleFormat::AV_SAMPLE_FMT_FLT,
                self.rate as c_int,
                &(*f).ch_layout,
                in_fmt,
                (*f).sample_rate,
                0,
                ptr::null_mut(),
            );
            if r < 0 || sys::swr_init(self.swr) < 0 {
                return Err(MediaError::Unsupported(format!("audio resampling: {}", err(r))));
            }
        }
        Ok(())
    }
}

impl Drop for AudioDec {
    fn drop(&mut self) {
        unsafe { sys::swr_free(&mut self.swr) };
    }
}

impl AudioDecoder for AudioDec {
    fn seek(&mut self, t: Time) -> Result<(), MediaError> {
        self.codec.seek(&self.input, self.stream, t)?;
        unsafe { sys::swr_free(&mut self.swr) }; // drop buffered samples
        self.skip_until = Some(t);
        Ok(())
    }

    fn next_block(&mut self) -> Result<Option<AudioBlock>, MediaError> {
        loop {
            if !self.codec.next(&self.input, self.stream)? {
                return Ok(None);
            }
            self.resampler()?;
            let f = self.codec.frame;
            let ch = self.channels as usize;
            let (pts, n) = unsafe {
                let ts = if (*f).best_effort_timestamp != NOPTS { (*f).best_effort_timestamp } else { (*f).pts };
                (to_time(ts, self.tb), (*f).nb_samples)
            };
            let cap = unsafe { sys::swr_get_out_samples(self.swr, n) }.max(0) as usize;
            let mut samples = vec![0f32; cap * ch];
            let got = unsafe {
                let out = samples.as_mut_ptr() as *mut u8;
                sys::swr_convert(self.swr, &out, cap as c_int, (*f).extended_data as *const *const u8, n)
            };
            unsafe { sys::av_frame_unref(f) };
            if got < 0 {
                return Err(MediaError::Corrupt(err(got)));
            }
            samples.truncate(got as usize * ch);
            let mut pts = pts;
            if let Some(target) = self.skip_until {
                let block_len = Time::from_seconds_f64(got as f64 / self.rate as f64);
                if pts + block_len <= target {
                    continue;
                }
                // Trim the part of this block before the target.
                let skip = ((target - pts).as_seconds_f64() * self.rate as f64).round().max(0.0) as usize;
                let skip = skip.min(got as usize);
                samples.drain(..skip * ch);
                pts = target;
                self.skip_until = None;
            }
            if samples.is_empty() {
                continue;
            }
            return Ok(Some(AudioBlock { pts, sample_rate: self.rate, channels: self.channels, samples }));
        }
    }
}
