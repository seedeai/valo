//! The zero-copy path: rasterise into a CoreVideo buffer that Metal and wgpu both retain.
//!
//! Core Video allocates an `IOSurface`-backed pixel buffer; Core Graphics draws into it on the
//! CPU; a `CVMetalTexture` views the same memory as a Metal texture; wgpu wraps that texture and
//! runs a release callback — which drops the CoreVideo objects — once the GPU is done with it.
use crate::raster::{draw_into, Destination};
use core_foundation::base::{CFType, TCFType};
use core_foundation::boolean::CFBoolean;
use core_foundation::dictionary::CFDictionary;
use core_foundation::number::CFNumber;
use core_foundation::string::CFString;
use core_graphics::image::CGImage;
use objc2::rc::Retained;
use objc2_core_foundation::CFRetained;
use objc2_core_video::*;
use objc2_metal::{MTLPixelFormat, MTLTextureUsage};
use std::ptr::{self, NonNull};
use valo::{ImageError, PixelFormat};
use valo_codec::DecodeError;

/// SharedError separates "use the CPU path" from "give up".
pub(crate) enum SharedError {
    /// Unavailable means this device or buffer cannot be shared; rasterise to CPU memory instead.
    Unavailable,
    /// Fatal means the decode itself failed; another path would fail the same way.
    Fatal(DecodeError),
}

impl From<DecodeError> for SharedError {
    fn from(error: DecodeError) -> Self {
        Self::Fatal(error)
    }
}

impl From<ImageError> for SharedError {
    fn from(error: ImageError) -> Self {
        match error {
            ImageError::UnsupportedBackend | ImageError::IncompatibleTexture => Self::Unavailable,
            other => Self::Fatal(DecodeError::Image(other)),
        }
    }
}

/// `rasterize_to_metal` draws `image` at `size` into a texture the device samples directly.
pub(crate) fn rasterize_to_metal(
    image: &CGImage,
    size: [u32; 2],
    device: &wgpu::Device,
) -> Result<wgpu::Texture, SharedError> {
    let metal_device = metal_device_of(device)?;
    let pixel_buffer = create_pixel_buffer(size)?;
    draw_into_pixel_buffer(image, size, &pixel_buffer)?;
    let metal_texture = metal_texture_of(&pixel_buffer, size, &metal_device)?;
    let raw = CVMetalTextureGetTexture(&metal_texture).ok_or(SharedError::Unavailable)?;
    let raw = NonNull::new(Retained::as_ptr(&raw).cast_mut().cast()).expect("retained texture");
    let backing = FrozenBacking {
        _pixel_buffer: pixel_buffer,
        _metal_texture: metal_texture,
    };
    // Safety: Core Graphics finished drawing before the buffer was unlocked; nothing writes to it
    // afterwards, and `backing` keeps both CoreVideo objects alive until wgpu releases the
    // texture, which is after its last GPU use.
    Ok(unsafe { valo::import_metal_texture(device, raw, Box::new(move || drop(backing))) }?)
}

/// Only ownership crosses threads: CPU writes and attachment mutation ended before sealing.
/// CoreVideo permits retaining these buffers through a Metal completion handler, which is what
/// wgpu's release callback amounts to. No buffer reference or writable address is exposed.
struct FrozenBacking {
    _pixel_buffer: CFRetained<CVPixelBuffer>,
    _metal_texture: CFRetained<CVMetalTexture>,
}
// Safety: the backing only releases CF ownership after GPU use and never touches the buffers.
unsafe impl Send for FrozenBacking {}
unsafe impl Sync for FrozenBacking {}

fn metal_device_of(
    device: &wgpu::Device,
) -> Result<Retained<objc2::runtime::ProtocolObject<dyn objc2_metal::MTLDevice>>, SharedError> {
    let hal =
        unsafe { device.as_hal::<wgpu::hal::api::Metal>() }.ok_or(SharedError::Unavailable)?;
    Ok(hal.raw_device().clone())
}

fn create_pixel_buffer(size: [u32; 2]) -> Result<CFRetained<CVPixelBuffer>, SharedError> {
    let attributes = pixel_buffer_attributes();
    let mut raw = ptr::null_mut();
    check(unsafe {
        CVPixelBufferCreate(
            None,
            size[0] as usize,
            size[1] as usize,
            kCVPixelFormatType_32BGRA,
            Some(as_cf_dictionary(&attributes)),
            NonNull::from(&mut raw),
        )
    })?;
    retained(raw)
}

fn draw_into_pixel_buffer(
    image: &CGImage,
    size: [u32; 2],
    pixel_buffer: &CVPixelBuffer,
) -> Result<(), SharedError> {
    check(unsafe { CVPixelBufferLockBaseAddress(pixel_buffer, CVPixelBufferLockFlags::empty()) })?;
    let _unlock = UnlockOnDrop(pixel_buffer);
    let pixels = CVPixelBufferGetBaseAddress(pixel_buffer);
    if pixels.is_null() {
        return Err(SharedError::Unavailable);
    }
    draw_into(
        image,
        Destination {
            pixels: pixels.cast(),
            size,
            row_bytes: CVPixelBufferGetBytesPerRow(pixel_buffer),
            format: PixelFormat::Bgra8,
        },
    )?;
    Ok(())
}

fn metal_texture_of(
    pixel_buffer: &CVPixelBuffer,
    size: [u32; 2],
    device: &objc2::runtime::ProtocolObject<dyn objc2_metal::MTLDevice>,
) -> Result<CFRetained<CVMetalTexture>, SharedError> {
    let mut raw_cache = ptr::null_mut();
    check(unsafe {
        CVMetalTextureCache::create(None, None, device, None, NonNull::from(&mut raw_cache))
    })?;
    let cache: CFRetained<CVMetalTextureCache> = retained(raw_cache)?;
    let usage = unsafe {
        CFDictionary::from_CFType_pairs(&[(
            cf_key(kCVMetalTextureUsage),
            CFNumber::from(MTLTextureUsage::ShaderRead.0 as i64).as_CFType(),
        )])
    };
    let mut raw = ptr::null_mut();
    check(unsafe {
        CVMetalTextureCache::create_texture_from_image(
            None,
            &cache,
            pixel_buffer,
            Some(as_cf_dictionary(&usage)),
            MTLPixelFormat::BGRA8Unorm,
            size[0] as usize,
            size[1] as usize,
            0,
            NonNull::from(&mut raw),
        )
    })?;
    retained(raw)
}

struct UnlockOnDrop<'a>(&'a CVPixelBuffer);

impl Drop for UnlockOnDrop<'_> {
    fn drop(&mut self) {
        unsafe {
            CVPixelBufferUnlockBaseAddress(self.0, CVPixelBufferLockFlags::empty());
        }
    }
}

/// IOSurface backing plus Metal, CGImage and bitmap-context compatibility, so the same
/// allocation can be drawn into by Core Graphics and sampled by Metal.
fn pixel_buffer_attributes() -> CFDictionary<CFType, CFType> {
    let empty = CFDictionary::<CFString, CFType>::from_CFType_pairs(&[]);
    unsafe {
        CFDictionary::from_CFType_pairs(&[
            (
                cf_key(kCVPixelBufferIOSurfacePropertiesKey),
                empty.as_CFType(),
            ),
            (
                cf_key(kCVPixelBufferMetalCompatibilityKey),
                CFBoolean::true_value().as_CFType(),
            ),
            (
                cf_key(kCVPixelBufferCGImageCompatibilityKey),
                CFBoolean::true_value().as_CFType(),
            ),
            (
                cf_key(kCVPixelBufferCGBitmapContextCompatibilityKey),
                CFBoolean::true_value().as_CFType(),
            ),
        ])
    }
}

/// Bridges an objc2 CoreFoundation string constant to the core-foundation crate's type.
unsafe fn cf_key(name: &objc2_core_foundation::CFString) -> CFType {
    let pointer: *const objc2_core_foundation::CFString = name;
    unsafe { CFString::wrap_under_get_rule(pointer.cast()) }.as_CFType()
}

fn as_cf_dictionary(
    dictionary: &CFDictionary<CFType, CFType>,
) -> &objc2_core_foundation::CFDictionary {
    unsafe {
        &*dictionary
            .as_concrete_TypeRef()
            .cast::<objc2_core_foundation::CFDictionary>()
    }
}

fn retained<T: objc2_core_foundation::Type>(raw: *mut T) -> Result<CFRetained<T>, SharedError> {
    let pointer = NonNull::new(raw).ok_or_else(|| SharedError::Fatal(allocation_failed()))?;
    Ok(unsafe { CFRetained::from_raw(pointer) })
}

fn check(status: CVReturn) -> Result<(), SharedError> {
    if status == kCVReturnSuccess {
        Ok(())
    } else if status == kCVReturnAllocationFailed {
        Err(SharedError::Fatal(allocation_failed()))
    } else {
        Err(SharedError::Unavailable)
    }
}

fn allocation_failed() -> DecodeError {
    DecodeError::Failed("CoreVideo could not allocate the pixel buffer".into())
}
