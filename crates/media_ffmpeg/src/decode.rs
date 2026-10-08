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

use crate::{built_hw_types, err, shared_device, sys, to_time, Input};

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
                if let Some((dev, fmt)) = hw_device_for(codec) {
                    (*ctx).hw_device_ctx = dev;
                    // `pick_hw_format` reads which format this device decodes to.
                    (*ctx).opaque = fmt as i32 as isize as *mut std::ffi::c_void;
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

/// Hardware decoders, best first. Each is tried only if this FFmpeg build
/// includes it, the codec can use it, and the machine's driver opens it — so
/// one list serves every platform: VideoToolbox on Apple; D3D12/D3D11 (any
/// GPU) or NVDEC (CUDA) on Windows; VA-API (Intel/AMD), NVDEC or Vulkan Video
/// on Linux; DXVA2 on old Windows. Decoded frames are copied to system memory
/// (`av_hwframe_transfer_data`), which works the same with a discrete GPU's
/// own memory or with unified memory.
const HW_PREFERENCE: [sys::AVHWDeviceType; 7] = {
    use sys::AVHWDeviceType::*;
    [
        AV_HWDEVICE_TYPE_VIDEOTOOLBOX,
        AV_HWDEVICE_TYPE_D3D12VA,
        AV_HWDEVICE_TYPE_D3D11VA,
        AV_HWDEVICE_TYPE_CUDA,
        AV_HWDEVICE_TYPE_VAAPI,
        AV_HWDEVICE_TYPE_VULKAN,
        AV_HWDEVICE_TYPE_DXVA2,
    ]
};

/// The best hardware device that can decode with `codec`, and the pixel
/// format it decodes to.
unsafe fn hw_device_for(codec: *const sys::AVCodec) -> Option<(*mut sys::AVBufferRef, sys::AVPixelFormat)> {
    let built = built_hw_types();
    for t in HW_PREFERENCE.into_iter().filter(|t| built.contains(t)) {
        let mut i = 0;
        let fmt = loop {
            let cfg = unsafe { sys::avcodec_get_hw_config(codec, i) };
            if cfg.is_null() {
                break None;
            }
            let cfg = unsafe { &*cfg };
            if cfg.device_type == t && cfg.methods & sys::AV_CODEC_HW_CONFIG_METHOD_HW_DEVICE_CTX as c_int != 0 {
                break Some(cfg.pix_fmt);
            }
            i += 1;
        };
        if let (Some(fmt), Some(dev)) = (fmt, fmt.and_then(|_| shared_device(t))) {
            return Some((dev, fmt));
        }
    }
    None
}

/// The hardware format chosen in `hw_device_for` if offered; otherwise
/// (a profile the hardware cannot do) the first software format.
unsafe extern "C" fn pick_hw_format(ctx: *mut sys::AVCodecContext, fmts: *const sys::AVPixelFormat) -> sys::AVPixelFormat {
    let wanted = unsafe { (*ctx).opaque as isize as i32 };
    let mut p = fmts;
    let mut software = None;
    unsafe {
        while *p != sys::AVPixelFormat::AV_PIX_FMT_NONE {
            if *p as i32 == wanted {
                return *p;
            }
            let d = sys::av_pix_fmt_desc_get(*p);
            if software.is_none() && !d.is_null() && (*d).flags & sys::AV_PIX_FMT_FLAG_HWACCEL as u64 == 0 {
                software = Some(*p);
            }
            p = p.add(1);
        }
        software.unwrap_or(*fmts)
    }
}

fn first_stream(input: &Input, kind: sys::AVMediaType) -> Option<usize> {
    input.streams().iter().position(|&s| unsafe { (*(*s).codecpar).codec_type == kind })
}

/// A decoded frame's two planes, shared with FFmpeg: holds a reference to
/// the frame's buffers and frees it when the engine is done with it.
struct AvPlanes {
    frame: *mut sys::AVFrame,
    rows: [usize; 2],
}

// The frame is not touched after it is handed out (read-only), and FFmpeg's
// buffer references are atomically counted.
unsafe impl Send for AvPlanes {}
unsafe impl Sync for AvPlanes {}

impl AvPlanes {
    /// `None` (and `frame` freed) when the frame cannot be shared as is.
    unsafe fn new(frame: *mut sys::AVFrame, h: u32) -> Option<AvPlanes> {
        if frame.is_null() {
            return None;
        }
        let ok = unsafe { (0..2).all(|i| (*frame).linesize[i] > 0 && !(*frame).data[i].is_null() && !(*frame).buf[0].is_null()) };
        if !ok {
            let mut f = frame;
            unsafe { sys::av_frame_free(&mut f) };
            return None;
        }
        Some(AvPlanes { frame, rows: [h as usize, h.div_ceil(2) as usize] })
    }
}

impl SharedPlanes for AvPlanes {
    fn planes(&self) -> Vec<&[u8]> {
        (0..2).map(|i| unsafe { std::slice::from_raw_parts((*self.frame).data[i], (*self.frame).linesize[i] as usize * self.rows[i]) }).collect()
    }
    fn strides(&self) -> Vec<usize> {
        (0..2).map(|i| unsafe { (*self.frame).linesize[i] as usize }).collect()
    }
}

impl Drop for AvPlanes {
    fn drop(&mut self) {
        unsafe { sys::av_frame_free(&mut self.frame) };
    }
}

/// A VideoToolbox frame left in GPU memory: a reference to the decoder's
/// `CVPixelBuffer` (`data[3]`), released when the engine drops the frame.
struct AvNative {
    frame: *mut sys::AVFrame,
}

// Read-only once handed out; CoreVideo buffers and FFmpeg references are
// thread-safe.
unsafe impl Send for AvNative {}
unsafe impl Sync for AvNative {}

impl NativeFrame for AvNative {
    fn handle(&self) -> NativeHandle {
        NativeHandle::CvPixelBuffer(unsafe { (*self.frame).data[3] } as *mut std::ffi::c_void)
    }

    fn to_cpu(&self) -> Option<(Vec<Vec<u8>>, Vec<usize>)> {
        unsafe {
            let mut sw = sys::av_frame_alloc();
            let ok = sys::av_hwframe_transfer_data(sw, self.frame, 0) >= 0 && (0..2).all(|i| (*sw).linesize[i] > 0);
            let out = ok.then(|| {
                let h = (*sw).height as usize;
                let rows = [h, h.div_ceil(2)];
                (0..2)
                    .map(|i| {
                        let stride = (*sw).linesize[i] as usize;
                        (std::slice::from_raw_parts((*sw).data[i], stride * rows[i]).to_vec(), stride)
                    })
                    .unzip()
            });
            sys::av_frame_free(&mut sw);
            out
        }
    }
}

impl Drop for AvNative {
    fn drop(&mut self) {
        unsafe { sys::av_frame_free(&mut self.frame) };
    }
}

pub struct VideoDec {
    codec: Codec,
    input: Input,
    stream: usize,
    tb: sys::AVRational,
    /// The file's start: subtracted from every timestamp.
    origin: Time,
    frame_dur: Time,
    sw: *mut sys::AVFrame,
    sws: *mut sys::SwsContext,
    skip_until: Option<Time>,
    /// The last frame skipped on the way to a seek target, kept so a seek
    /// past the end still yields a picture (the last one).
    held: *mut sys::AVFrame,
    /// Where the last seek went in the file, and how far back the next
    /// retry goes, while a seek is being made good (see `retry`).
    sought: Time,
    backoff: Time,
    /// Hand hardware frames out as they are (in GPU memory) when the
    /// consumer can import them; see [`AvNative`].
    native: bool,
    pub hardware: bool,
}

// The FFmpeg objects are owned exclusively by this decoder.
unsafe impl Send for VideoDec {}

impl VideoDec {
    pub fn open(media: &Resolved, hw: bool) -> Result<VideoDec, MediaError> {
        Self::open_with(media, hw, false)
    }

    /// `native`: leave hardware-decoded frames in GPU memory when they can be
    /// imported as they are (4:2:0, 8 or 10 bit), instead of copying them out.
    pub fn open_with(media: &Resolved, hw: bool, native: bool) -> Result<VideoDec, MediaError> {
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
        let origin = input.origin();
        Ok(VideoDec {
            codec,
            input,
            stream,
            tb,
            origin,
            frame_dur: rate.frame_duration(),
            sw: unsafe { sys::av_frame_alloc() },
            sws: ptr::null_mut(),
            skip_until: None,
            held: unsafe { sys::av_frame_alloc() },
            sought: Time::ZERO,
            backoff: Time::ZERO,
            native,
            hardware,
        })
    }

    /// Decoded frame `decoded` as NV12 or P010, copied out of FFmpeg.
    fn convert(&mut self, decoded: *mut sys::AVFrame) -> Result<VideoFrame, MediaError> {
        if self.native {
            if let Some(f) = unsafe { self.native_frame(decoded) } {
                return Ok(f);
            }
        }
        unsafe {
            let mut src = decoded;
            if (*src).format == sys::AVPixelFormat::AV_PIX_FMT_VIDEOTOOLBOX as c_int {
                sys::av_frame_unref(self.sw);
                let r = sys::av_hwframe_transfer_data(self.sw, src, 0);
                if r < 0 {
                    return Err(MediaError::Other(format!("hardware frame transfer: {}", err(r))));
                }
                // The transfer moves pixels only: bring the colour tags along.
                sys::av_frame_copy_props(self.sw, src);
                src = self.sw;
            }
            let fmt: sys::AVPixelFormat = std::mem::transmute((*src).format);
            let desc = sys::av_pix_fmt_desc_get(fmt);
            let deep = !desc.is_null() && (*desc).comp[0].depth > 8;
            let rgb = !desc.is_null() && (*desc).flags & sys::AV_PIX_FMT_FLAG_RGB as u64 != 0;
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
                // Say exactly what the YUV should be, so the tags handed on
                // describe the pixels: RGB becomes BT.709 limited range;
                // YUV keeps its matrix and range (a full-range source stays
                // full range). Primaries and transfer are never converted.
                (*dst).color_primaries = (*src).color_primaries;
                (*dst).color_trc = (*src).color_trc;
                if rgb {
                    (*dst).colorspace = sys::AVColorSpace::AVCOL_SPC_BT709;
                    (*dst).color_range = sys::AVColorRange::AVCOL_RANGE_MPEG;
                } else {
                    (*dst).colorspace = (*src).colorspace;
                    (*dst).color_range = match (*src).color_range {
                        sys::AVColorRange::AVCOL_RANGE_UNSPECIFIED => sys::AVColorRange::AVCOL_RANGE_MPEG,
                        r => r,
                    };
                }
                let r = sys::sws_scale_frame(self.sws, dst, src);
                if r < 0 {
                    sys::av_frame_free(&mut dst);
                    return Err(MediaError::Other(format!("pixel conversion: {}", err(r))));
                }
                dst
            };
            let (w, h) = ((*ready).width as u32, (*ready).height as u32);
            let f = decoded;
            let pts = if (*f).best_effort_timestamp != NOPTS { (*f).best_effort_timestamp } else { (*f).pts };
            let dur = if (*f).duration > 0 { to_time((*f).duration, self.tb) } else { self.frame_dur };
            // The tags of the pixels handed out: the converted frame's when
            // there was a conversion.
            let tagged = ready;
            let color = ColorTags {
                primaries: name(sys::av_color_primaries_name((*tagged).color_primaries)),
                transfer: name(sys::av_color_transfer_name((*tagged).color_trc)),
                matrix: name(sys::av_color_space_name((*tagged).colorspace)),
                full_range: (*tagged).color_range == sys::AVColorRange::AVCOL_RANGE_JPEG,
            };
            // Hand the planes out without copying them: keep a reference to
            // FFmpeg's (reference-counted) buffers for as long as the frame
            // lives. A converted frame is ours already.
            let owned = if !dst.is_null() {
                std::mem::replace(&mut dst, ptr::null_mut())
            } else {
                let r = sys::av_frame_alloc();
                if sys::av_frame_ref(r, ready) < 0 {
                    let mut r = r;
                    sys::av_frame_free(&mut r);
                    ptr::null_mut()
                } else {
                    r
                }
            };
            let data = match AvPlanes::new(owned, h) {
                Some(p) => FrameData::Shared(Box::new(p)),
                // Not shareable (no reference counting, bottom-up rows): copy.
                None => {
                    let rows = [h as usize, h.div_ceil(2) as usize];
                    let mut planes = Vec::with_capacity(2);
                    let mut strides = Vec::with_capacity(2);
                    for (i, &n) in rows.iter().enumerate() {
                        let stride = (*ready).linesize[i].unsigned_abs() as usize;
                        planes.push(std::slice::from_raw_parts((*ready).data[i], stride * n).to_vec());
                        strides.push(stride);
                    }
                    FrameData::Cpu { planes, strides }
                }
            };
            if !dst.is_null() {
                sys::av_frame_free(&mut dst);
            }
            Ok(VideoFrame {
                pts: to_time(pts, self.tb) - self.origin,
                duration: dur,
                width: w,
                height: h,
                format: out_fmt,
                color,
                data,
            })
        }
    }
}

impl VideoDec {
    /// `decoded` left where the hardware put it, if it is a picture the
    /// compositor can import as is.
    unsafe fn native_frame(&self, decoded: *mut sys::AVFrame) -> Option<VideoFrame> {
        unsafe {
            let f = decoded;
            if (*f).format != sys::AVPixelFormat::AV_PIX_FMT_VIDEOTOOLBOX as c_int || (*f).hw_frames_ctx.is_null() {
                return None;
            }
            let fc = (*(*f).hw_frames_ctx).data as *const sys::AVHWFramesContext;
            let format = match (*fc).sw_format {
                sys::AVPixelFormat::AV_PIX_FMT_NV12 => PixelFormat::Nv12,
                sys::AVPixelFormat::AV_PIX_FMT_P010LE => PixelFormat::P010,
                _ => return None, // 4:2:2 / 4:4:4: the copy path converts it
            };
            let held = sys::av_frame_alloc();
            if sys::av_frame_ref(held, f) < 0 {
                let mut h = held;
                sys::av_frame_free(&mut h);
                return None;
            }
            let pts = if (*f).best_effort_timestamp != NOPTS { (*f).best_effort_timestamp } else { (*f).pts };
            Some(VideoFrame {
                pts: to_time(pts, self.tb) - self.origin,
                duration: if (*f).duration > 0 { to_time((*f).duration, self.tb) } else { self.frame_dur },
                width: (*f).width as u32,
                height: (*f).height as u32,
                format,
                color: ColorTags {
                    primaries: name(sys::av_color_primaries_name((*f).color_primaries)),
                    transfer: name(sys::av_color_transfer_name((*f).color_trc)),
                    matrix: name(sys::av_color_space_name((*f).colorspace)),
                    full_range: (*f).color_range == sys::AVColorRange::AVCOL_RANGE_JPEG,
                },
                data: FrameData::Native(Box::new(AvNative { frame: held })),
            })
        }
    }

    /// While a seek target has produced nothing at or before it, seek
    /// further back (1 s, 2 s, 4 s … down to the file's start) and decode
    /// forward again. Some demuxers (MPEG-TS) seek to any packet, not a key
    /// frame, so decoding from there yields nothing until the next one.
    fn retry(&mut self) -> Result<bool, MediaError> {
        let Some(target) = self.skip_until else { return Ok(false) };
        if unsafe { !(*self.held).buf[0].is_null() } || self.sought <= Time::ZERO {
            return Ok(false);
        }
        self.sought = (target - self.backoff).max(Time::ZERO);
        self.backoff = self.backoff + self.backoff;
        self.codec.seek(&self.input, self.stream, self.sought + self.origin)?;
        Ok(true)
    }
}

impl Drop for VideoDec {
    fn drop(&mut self) {
        unsafe {
            sys::av_frame_free(&mut self.sw);
            sys::av_frame_free(&mut self.held);
            if !self.sws.is_null() {
                sys::sws_free_context(&mut self.sws);
            }
        }
    }
}

impl VideoDecoder for VideoDec {
    fn seek(&mut self, t: Time) -> Result<(), MediaError> {
        self.codec.seek(&self.input, self.stream, t + self.origin)?;
        self.skip_until = Some(t);
        self.sought = t;
        self.backoff = Time::from_seconds(1);
        unsafe { sys::av_frame_unref(self.held) };
        Ok(())
    }

    fn next_frame(&mut self) -> Result<Option<VideoFrame>, MediaError> {
        loop {
            if !self.codec.next(&self.input, self.stream)? {
                if self.retry()? {
                    continue;
                }
                // Sought past the end: the last frame there is.
                if self.skip_until.take().is_some() && unsafe { !(*self.held).buf[0].is_null() } {
                    let out = self.convert(self.held);
                    unsafe { sys::av_frame_unref(self.held) };
                    return out.map(Some);
                }
                return Ok(None);
            }
            let f = self.codec.frame;
            let pts = unsafe { if (*f).best_effort_timestamp != NOPTS { (*f).best_effort_timestamp } else { (*f).pts } };
            let start = to_time(pts, self.tb) - self.origin;
            if let Some(target) = self.skip_until {
                // The seek landed after the target (a container that does
                // not seek to key frames): go back further and decode on.
                if start > target && self.retry()? {
                    unsafe { sys::av_frame_unref(f) };
                    continue;
                }
                // Frames that end before the target are decoded but not returned.
                if start + self.frame_dur <= target {
                    unsafe {
                        sys::av_frame_unref(self.held);
                        sys::av_frame_move_ref(self.held, f);
                    }
                    continue;
                }
                self.skip_until = None;
                unsafe { sys::av_frame_unref(self.held) };
            }
            let out = self.convert(f);
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
    /// The file's start: subtracted from every timestamp.
    origin: Time,
}

unsafe impl Send for AudioDec {}

/// Longest silence put before audio that starts after a seek target.
const MAX_LEAD_IN: Time = Time::from_seconds(10);

impl AudioDec {
    pub fn open(media: &Resolved, rate: u32, channels: u16) -> Result<AudioDec, MediaError> {
        let input = Input::open(media)?;
        let stream = first_stream(&input, sys::AVMediaType::AVMEDIA_TYPE_AUDIO).ok_or_else(|| MediaError::Unsupported("no audio stream".into()))?;
        let (codec, _) = Codec::open(&input, stream, false)?;
        let tb = unsafe { (*input.streams()[stream]).time_base };
        let origin = input.origin();
        Ok(AudioDec { codec, input, stream, tb, swr: ptr::null_mut(), rate, channels, skip_until: None, origin })
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
        self.codec.seek(&self.input, self.stream, t + self.origin)?;
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
                (to_time(ts, self.tb) - self.origin, (*f).nb_samples)
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
                if pts < target {
                    // Trim the part of this block before the target.
                    let skip = ((target - pts).as_seconds_f64() * self.rate as f64).round() as usize;
                    samples.drain(..skip.min(got as usize) * ch);
                } else {
                    // The sound starts after the target: silence until it does.
                    let lead = ((pts - target).min(MAX_LEAD_IN).as_seconds_f64() * self.rate as f64).round() as usize;
                    samples.splice(0..0, std::iter::repeat_n(0.0, lead * ch));
                }
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
