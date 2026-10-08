//! Platform GPU interop for the compositor: frames that a hardware decoder
//! left in GPU memory become textures on the compositor's device without a
//! copy ([`ve_render::TextureImporter`]).
//!
//! - **Apple**: VideoToolbox's `CVPixelBuffer`s → Metal textures through a
//!   `CVMetalTextureCache`. The buffers are IOSurfaces, so this works with
//!   unified memory and with a discrete GPU's own memory (Intel Macs with
//!   AMD GPUs), where the driver moves the surface to VRAM as needed.
//! - **Elsewhere**: no importer yet. Hardware-decoded frames are copied to
//!   system memory by the decoder and uploaded, which works on any GPU.
//!   D3D11/D3D12 shared textures (Windows) and DMA-BUF / Vulkan images
//!   (Linux) are the interop that would go here.

use std::sync::Arc;
use ve_render::TextureImporter;

/// This platform's importer, if it has one.
pub fn importer() -> Option<Arc<dyn TextureImporter>> {
    #[cfg(target_vendor = "apple")]
    {
        Some(Arc::new(apple::MetalImporter::default()))
    }
    #[cfg(not(target_vendor = "apple"))]
    {
        None
    }
}

#[cfg(target_vendor = "apple")]
mod apple {
    use std::ptr::NonNull;
    use std::sync::Mutex;

    use objc2_core_foundation::CFRetained;
    use objc2_core_video::{
        kCVReturnSuccess, CVMetalTexture, CVMetalTextureCache, CVMetalTextureGetTexture, CVPixelBuffer, CVPixelBufferGetHeightOfPlane,
        CVPixelBufferGetWidthOfPlane,
    };
    use objc2_metal::{MTLPixelFormat, MTLTextureType};
    use ve_ports::{FrameData, NativeHandle, PixelFormat, VideoFrame};

    /// A CoreFoundation object handed between threads. CoreVideo's texture
    /// caches and textures are thread-safe; Rust just cannot see it.
    struct Sendable<T>(T);
    unsafe impl<T> Send for Sendable<T> {}
    unsafe impl<T> Sync for Sendable<T> {}

    #[derive(Default)]
    pub struct MetalImporter {
        /// One cache per Metal device (by address), made on first use.
        cache: Mutex<Option<(usize, Sendable<CFRetained<CVMetalTextureCache>>)>>,
    }

    impl ve_render::TextureImporter for MetalImporter {
        fn import(&self, device: &wgpu::Device, frame: &VideoFrame) -> Option<[wgpu::Texture; 2]> {
            let FrameData::Native(native) = &frame.data else { return None };
            #[allow(irrefutable_let_patterns)]
            let NativeHandle::CvPixelBuffer(pb) = native.handle() else { return None };
            if pb.is_null() {
                return None;
            }
            // Valid while `frame` lives; the textures keep their own references.
            let pb: &CVPixelBuffer = unsafe { &*(pb as *const CVPixelBuffer) };
            let deep = match frame.format {
                PixelFormat::Nv12 => false,
                PixelFormat::P010 if device.features().contains(wgpu::Features::TEXTURE_FORMAT_16BIT_NORM) => true,
                _ => return None,
            };
            let (mtl, wgf) = if deep {
                ([MTLPixelFormat::R16Unorm, MTLPixelFormat::RG16Unorm], [wgpu::TextureFormat::R16Unorm, wgpu::TextureFormat::Rg16Unorm])
            } else {
                ([MTLPixelFormat::R8Unorm, MTLPixelFormat::RG8Unorm], [wgpu::TextureFormat::R8Unorm, wgpu::TextureFormat::Rg8Unorm])
            };
            // Not a Metal device (another backend was chosen): nothing to do.
            let hal = unsafe { device.as_hal::<wgpu::hal::api::Metal>() }?;
            let raw = hal.raw_device();
            let key = &**raw as *const _ as *const () as usize;

            let mut guard = self.cache.lock().unwrap();
            if guard.as_ref().is_none_or(|(k, _)| *k != key) {
                let mut out: *mut CVMetalTextureCache = std::ptr::null_mut();
                let r = unsafe { CVMetalTextureCache::create(None, None, raw, None, NonNull::from(&mut out)) };
                if r != kCVReturnSuccess || out.is_null() {
                    return None;
                }
                *guard = Some((key, Sendable(unsafe { CFRetained::from_raw(NonNull::new_unchecked(out)) })));
            }
            let cache = &guard.as_ref().unwrap().1 .0;
            // Let go of textures nothing uses any more.
            cache.flush(0);

            let mut planes = Vec::with_capacity(2);
            for plane in 0..2 {
                let (w, h) = (CVPixelBufferGetWidthOfPlane(pb, plane), CVPixelBufferGetHeightOfPlane(pb, plane));
                let mut out: *mut CVMetalTexture = std::ptr::null_mut();
                let r = unsafe { CVMetalTextureCache::create_texture_from_image(None, cache, pb, None, mtl[plane], w, h, plane, NonNull::from(&mut out)) };
                if r != kCVReturnSuccess || out.is_null() {
                    return None;
                }
                let cv = unsafe { CFRetained::from_raw(NonNull::new_unchecked(out)) };
                let texture = CVMetalTextureGetTexture(&cv)?;
                // The CoreVideo texture (and through it the pixel buffer)
                // lives as long as wgpu's texture does.
                let keep = Sendable(cv);
                let extent = wgpu::hal::CopyExtent { width: w as u32, height: h as u32, depth: 1 };
                let hal_texture = unsafe {
                    wgpu::hal::metal::Device::texture_from_raw(texture, wgf[plane], MTLTextureType::Type2D, 1, 1, extent, Some(Box::new(move || drop(keep))))
                };
                let desc = wgpu::TextureDescriptor {
                    label: Some("decoded plane"),
                    size: wgpu::Extent3d { width: w as u32, height: h as u32, depth_or_array_layers: 1 },
                    mip_level_count: 1,
                    sample_count: 1,
                    dimension: wgpu::TextureDimension::D2,
                    format: wgf[plane],
                    usage: wgpu::TextureUsages::TEXTURE_BINDING,
                    view_formats: &[],
                };
                planes.push(unsafe { device.create_texture_from_hal::<wgpu::hal::api::Metal>(hal_texture, &desc, wgpu::TextureUses::RESOURCE) });
            }
            let chroma = planes.pop()?;
            Some([planes.pop()?, chroma])
        }
    }
}
