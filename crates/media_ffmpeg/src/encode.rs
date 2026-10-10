//! Encoding: H.264 / HEVC on the GPU's media engine — whichever this
//! machine has (see [`video_encoders`]) — ProRes 422 HQ through FFmpeg's own
//! (LGPL) encoder, AAC audio, into MP4 or MOV.
//!
//! Video arrives as display-encoded RGBA — 16 bits per channel from export,
//! 8 from elsewhere — and leaves as Rec.709 YCbCr at the codec's depth
//! (10-bit for ProRes and, where the encoder can, HEVC), tagged as such.

use std::ffi::{c_int, CStr, CString};
use std::ptr;

use ve_ports::*;

use crate::{err, shared_device, sys};

const EAGAIN: c_int = -(sys::EAGAIN as c_int);
const EOF: c_int = -((b'E' as c_int) | (b'O' as c_int) << 8 | (b'F' as c_int) << 16 | (b' ' as c_int) << 24);

struct Stream {
    ctx: *mut sys::AVCodecContext,
    st: *mut sys::AVStream,
    /// For encoders that take frames in GPU memory (VA-API, Vulkan): the
    /// pool frames are uploaded into. Null otherwise.
    hw_frames: *mut sys::AVBufferRef,
    /// The format frames are converted to in memory (before any upload).
    sw_pix: sys::AVPixelFormat,
}

/// Pixel formats to offer encoder `name`, deepest first: ProRes is 10-bit
/// 4:2:2; HEVC is 10-bit (Main 10) where the encoder can, else 8-bit; H.264
/// is 8-bit (10-bit H.264 is rarely supported by hardware or players).
fn pixel_formats(name: &str) -> &'static [sys::AVPixelFormat] {
    use sys::AVPixelFormat::*;
    if name == "prores_ks" {
        &[AV_PIX_FMT_YUV422P10LE]
    } else if name.starts_with("hevc") {
        &[AV_PIX_FMT_P010LE, AV_PIX_FMT_NV12]
    } else {
        &[AV_PIX_FMT_NV12]
    }
}

/// An encoder to try: FFmpeg name, the device type it needs frames uploaded
/// to (if any), and private options.
type Candidate = (&'static str, Option<sys::AVHWDeviceType>, &'static [(&'static str, &'static str)]);

/// Encoders for `codec`, best first. The first that this FFmpeg build has
/// and that opens on this machine is used, so one list serves Apple
/// (VideoToolbox), NVIDIA (NVENC), AMD (AMF, or Media Foundation on
/// Windows), Intel (QSV, Media Foundation) and Linux (VA-API, Vulkan Video)
/// — integrated or discrete GPU alike — and Windows' software encoder last.
/// All LGPL-compatible.
fn video_encoders(codec: &str) -> &'static [Candidate] {
    use sys::AVHWDeviceType::{AV_HWDEVICE_TYPE_VAAPI as VAAPI, AV_HWDEVICE_TYPE_VULKAN as VULKAN};
    const MF_HW: &[(&str, &str)] = &[("hw_encoding", "1")];
    match codec {
        "hevc" => &[
            ("hevc_videotoolbox", None, &[]),
            ("hevc_nvenc", None, &[]),
            ("hevc_amf", None, &[]),
            ("hevc_qsv", None, &[]),
            ("hevc_mf", None, MF_HW),
            ("hevc_vaapi", Some(VAAPI), &[]),
            ("hevc_vulkan", Some(VULKAN), &[]),
            // Windows' own software encoder (with the HEVC extension).
            ("hevc_mf", None, &[]),
        ],
        "prores" => &[("prores_ks", None, &[("profile", "hq")])],
        _ => &[
            ("h264_videotoolbox", None, &[]),
            ("h264_nvenc", None, &[]),
            ("h264_amf", None, &[]),
            ("h264_qsv", None, &[]),
            ("h264_mf", None, MF_HW),
            ("h264_vaapi", Some(VAAPI), &[]),
            ("h264_vulkan", Some(VULKAN), &[]),
            // Windows' own software encoder: machines with no GPU encoder.
            ("h264_mf", None, &[]),
        ],
    }
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
                video: Stream { ctx: ptr::null_mut(), st: ptr::null_mut(), hw_frames: ptr::null_mut(), sw_pix: sys::AVPixelFormat::AV_PIX_FMT_NV12 },
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
        let mut tried = Vec::new();
        for &(name, device, opts) in video_encoders(&s.video_codec) {
            for &pix in pixel_formats(name) {
                match unsafe { self.try_video(s, name, device, opts, pix) } {
                    Ok(st) => return Ok(st),
                    Err(e) => tried.push(format!("{name} ({pix:?}): {e}")),
                }
            }
        }
        Err(MediaError::Unsupported(format!("no {} encoder works here ({})", s.video_codec, tried.join("; "))))
    }

    /// Open encoder `name`, or say why not. Adds the stream only on success.
    unsafe fn try_video(
        &mut self,
        s: &EncoderSettings,
        name: &str,
        device: Option<sys::AVHWDeviceType>,
        opts: &[(&str, &str)],
        sw_pix: sys::AVPixelFormat,
    ) -> Result<Stream, String> {
        let cname = CString::new(name).unwrap();
        let codec = unsafe { sys::avcodec_find_encoder_by_name(cname.as_ptr()) };
        if codec.is_null() {
            return Err("not in this build".into());
        }
        unsafe {
            let mut ctx = sys::avcodec_alloc_context3(codec);
            (*ctx).width = s.width as c_int;
            (*ctx).height = s.height as c_int;
            (*ctx).time_base = sys::AVRational { num: s.rate.den as c_int, den: s.rate.num as c_int };
            (*ctx).framerate = sys::AVRational { num: s.rate.num as c_int, den: s.rate.den as c_int };
            (*ctx).pix_fmt = sw_pix;
            (*ctx).color_primaries = sys::AVColorPrimaries::AVCOL_PRI_BT709;
            (*ctx).color_trc = sys::AVColorTransferCharacteristic::AVCOL_TRC_BT709;
            (*ctx).colorspace = sys::AVColorSpace::AVCOL_SPC_BT709;
            (*ctx).color_range = sys::AVColorRange::AVCOL_RANGE_MPEG;
            let pixels = (s.width * s.height) as u64;
            (*ctx).bit_rate = s.video_bitrate.unwrap_or(if s.video_codec == "hevc" {
                pixels * 6 // ~12 Mb/s at 1080p
            } else {
                pixels * 10 // ~20 Mb/s at 1080p
            }) as i64;
            for (k, v) in opts {
                let (k, v) = (CString::new(*k).unwrap(), CString::new(*v).unwrap());
                sys::av_opt_set((*ctx).priv_data, k.as_ptr(), v.as_ptr(), 0);
            }
            if (*(*self.oc).oformat).flags & sys::AVFMT_GLOBALHEADER as c_int != 0 {
                (*ctx).flags |= sys::AV_CODEC_FLAG_GLOBAL_HEADER as c_int;
            }
            // Encoders that read frames from GPU memory get a pool to upload into.
            let mut hw_frames = ptr::null_mut();
            if let Some(t) = device {
                let Some(mut dev) = shared_device(t) else {
                    sys::avcodec_free_context(&mut ctx);
                    return Err("no such device here".into());
                };
                hw_frames = sys::av_hwframe_ctx_alloc(dev);
                sys::av_buffer_unref(&mut dev);
                let fc = (*hw_frames).data as *mut sys::AVHWFramesContext;
                (*fc).format = match t {
                    sys::AVHWDeviceType::AV_HWDEVICE_TYPE_VAAPI => sys::AVPixelFormat::AV_PIX_FMT_VAAPI,
                    _ => sys::AVPixelFormat::AV_PIX_FMT_VULKAN,
                };
                (*fc).sw_format = sw_pix;
                (*fc).width = s.width as c_int;
                (*fc).height = s.height as c_int;
                (*fc).initial_pool_size = 8;
                let r = sys::av_hwframe_ctx_init(hw_frames);
                if r < 0 {
                    sys::av_buffer_unref(&mut hw_frames);
                    sys::avcodec_free_context(&mut ctx);
                    return Err(format!("frame pool: {}", err(r)));
                }
                (*ctx).pix_fmt = (*fc).format;
                (*ctx).hw_frames_ctx = sys::av_buffer_ref(hw_frames);
            }
            let r = sys::avcodec_open2(ctx, codec, ptr::null_mut());
            if r < 0 {
                sys::av_buffer_unref(&mut hw_frames);
                sys::avcodec_free_context(&mut ctx);
                return Err(err(r));
            }
            let st = sys::avformat_new_stream(self.oc, ptr::null());
            let r = sys::avcodec_parameters_from_context((*st).codecpar, ctx);
            if r < 0 {
                sys::av_buffer_unref(&mut hw_frames);
                sys::avcodec_free_context(&mut ctx);
                return Err(format!("video parameters: {}", err(r)));
            }
            (*st).time_base = (*ctx).time_base;
            (*st).avg_frame_rate = (*ctx).framerate;
            Ok(Stream { ctx, st, hw_frames, sw_pix })
        }
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
        Ok(Stream { ctx, st, hw_frames: ptr::null_mut(), sw_pix: sys::AVPixelFormat::AV_PIX_FMT_NONE })
    }

    /// Send a frame (or `null` to flush) and write whatever comes out.
    unsafe fn send(&mut self, audio: bool, frame: *const sys::AVFrame) -> Result<(), MediaError> {
        let s = if audio { self.audio.as_ref().unwrap() } else { &self.video };
        let (ctx, st) = (s.ctx, s.st);
        let r = sys::avcodec_send_frame(ctx, frame);
        if r < 0 && r != EOF {
            // An encoder that opened but refuses the very first picture
            // (a hardware encoder without this profile, say) cannot do this
            // export here at all: say so as such, naming it.
            if !audio && self.frames <= 1 {
                let name = CStr::from_ptr((*(*ctx).codec).name).to_string_lossy();
                return Err(MediaError::Unsupported(format!("no {name} encoder works here ({name} rejected the first frame: {})", err(r))));
            }
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
        let Some(CpuPlanes { planes, strides }) = frame.data.cpu() else {
            return Err(MediaError::Unsupported("export frames must be in memory".into()));
        };
        let src_fmt = match frame.format {
            PixelFormat::Rgba8 => sys::AVPixelFormat::AV_PIX_FMT_RGBA,
            PixelFormat::Rgba16 => sys::AVPixelFormat::AV_PIX_FMT_RGBA64LE,
            _ => return Err(MediaError::Unsupported("export frames must be RGBA8 or RGBA16".into())),
        };
        unsafe {
            let ctx = self.video.ctx;
            let mut src = sys::av_frame_alloc();
            (*src).width = frame.width as c_int;
            (*src).height = frame.height as c_int;
            (*src).format = src_fmt as c_int;
            (*src).data[0] = planes[0].as_ptr() as *mut u8;
            (*src).linesize[0] = strides[0] as c_int;
            (*src).colorspace = sys::AVColorSpace::AVCOL_SPC_RGB;
            (*src).color_range = sys::AVColorRange::AVCOL_RANGE_JPEG;
            let mut dst = sys::av_frame_alloc();
            (*dst).width = (*ctx).width;
            (*dst).height = (*ctx).height;
            (*dst).format = self.video.sw_pix as c_int;
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
            // An encoder that reads GPU memory gets the frame uploaded first.
            if !self.video.hw_frames.is_null() {
                let mut hw = sys::av_frame_alloc();
                let mut r = sys::av_hwframe_get_buffer(self.video.hw_frames, hw, 0);
                if r >= 0 {
                    r = sys::av_hwframe_transfer_data(hw, dst, 0);
                }
                if r >= 0 {
                    sys::av_frame_copy_props(hw, dst);
                }
                sys::av_frame_free(&mut dst);
                if r < 0 {
                    sys::av_frame_free(&mut hw);
                    return Err(MediaError::Other(format!("upload to the encoder: {}", err(r))));
                }
                dst = hw;
            }
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
                sys::av_buffer_unref(&mut s.hw_frames);
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
