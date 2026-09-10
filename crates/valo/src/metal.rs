//! Metal interop: the one place a raw `MTLTexture*` becomes a wgpu texture.
//!
//! Two lifetimes meet here. A render target a host hands in stays the host's: wgpu only retains
//! it. A decoded image a native codec hands in comes with backing (an `IOSurface`, a
//! `CVPixelBuffer`) that must outlive every GPU command that samples it, so wgpu owns a release
//! callback and runs it after the last in-flight use.
use crate::ImageError;
use objc2::rc::Retained;
use objc2::runtime::ProtocolObject;
use objc2_metal::{MTLPixelFormat, MTLResource, MTLTexture, MTLTextureType, MTLTextureUsage};
use std::ffi::c_void;
use std::ptr::NonNull;
use valo_geometry::Color;
use valo_renderer::RenderTarget;

type RawTexture = ProtocolObject<dyn MTLTexture>;

/// `metal_device_of` returns the raw `MTLDevice*` behind a wgpu device.
///
/// Use it to configure a `CAMetalLayer` whose textures Valo will render into.
/// It returns `None` for non-Metal backends. The pointer is borrowed and
/// remains valid while `device` lives.
pub fn metal_device_of(device: &wgpu::Device) -> Option<NonNull<c_void>> {
    let hal_device = unsafe { device.as_hal::<wgpu::hal::api::Metal>() }?;
    let raw = Retained::as_ptr(hal_device.raw_device());
    NonNull::new(raw.cast_mut().cast())
}

/// `ExternalMetalTexture` wraps a caller-owned `MTLTexture` as a render target.
///
/// The embedder remains responsible for acquiring and presenting the texture.
pub struct ExternalMetalTexture {
    texture: wgpu::Texture,
    view: wgpu::TextureView,
    /// `format` is the wrapped texture's pixel format.
    pub format: wgpu::TextureFormat,
    /// `size` is the wrapped texture's dimensions in pixels.
    pub size: [u32; 2],
}

impl ExternalMetalTexture {
    /// `wrap` creates a render target from a raw `MTLTexture*`.
    ///
    /// Destination-reading blends and backdrop filters require copy access.
    /// For a `CAMetalLayer` drawable, set `framebufferOnly` to `false`.
    ///
    /// # Safety
    /// `texture` must point to a texture of exactly `size` and `format` created
    /// by the device returned from [`metal_device_of`].
    pub unsafe fn wrap(
        device: &wgpu::Device,
        texture: NonNull<c_void>,
        size: [u32; 2],
        format: wgpu::TextureFormat,
    ) -> Self {
        let texture = unsafe {
            wrap_metal_texture(
                device,
                texture,
                size,
                format,
                wgpu::TextureUsages::RENDER_ATTACHMENT | wgpu::TextureUsages::COPY_SRC,
            )
        };
        let view = texture.create_view(&wgpu::TextureViewDescriptor::default());
        Self {
            texture,
            view,
            format,
            size,
        }
    }

    /// `target` creates a render target over the wrapped texture.
    ///
    /// Pass `None` to preserve the texture's existing pixels.
    pub fn target(&self, clear: Option<Color>) -> RenderTarget<'_> {
        RenderTarget {
            view: &self.view,
            texture: &self.texture,
            format: self.format,
            size: self.size,
            clear,
        }
    }
}

/// `wrap_metal_texture` wraps a raw `MTLTexture*` as a wgpu texture the host keeps alive.
///
/// The returned texture retains the Metal texture. `usage` must not exceed
/// the usages with which the original texture was created.
///
/// # Safety
/// `texture` must point to a texture of exactly `size` and `format` created by
/// the device returned from [`metal_device_of`].
pub unsafe fn wrap_metal_texture(
    device: &wgpu::Device,
    texture: NonNull<c_void>,
    size: [u32; 2],
    format: wgpu::TextureFormat,
    usage: wgpu::TextureUsages,
) -> wgpu::Texture {
    let retained = unsafe { retain(texture) };
    let shape = TextureShape {
        format,
        size,
        mip_levels: 1,
        usage,
    };
    // wgpu 30 wants the state the texture arrives in. An imported target is
    // one valo is about to render into, and its previous contents are the
    // host's business, so COLOR_TARGET is the honest declaration —
    // UNINITIALIZED would license discarding pixels the host may still want.
    unsafe {
        texture_from_raw(
            device,
            retained,
            shape,
            None,
            wgpu::wgt::TextureUses::COLOR_TARGET,
        )
    }
}

/// `import_metal_texture` wraps a decoded `MTLTexture*` whose backing wgpu must keep alive.
///
/// Shape, format and mip count are read from the texture itself; it must be a 2D, single-sample
/// `RGBA8Unorm` or `BGRA8Unorm` texture with shader-read usage on `device`. `release` runs once
/// wgpu has finished every recorded and in-flight use, so capture whatever owns the pixels in it.
/// On a validation error `release` is dropped, which frees those captures immediately.
///
/// # Safety
/// `texture` must be a live `MTLTexture*`, all writes to it must have completed, and its contents
/// must stay immutable until `release` runs.
pub unsafe fn import_metal_texture(
    device: &wgpu::Device,
    texture: NonNull<c_void>,
    release: Box<dyn FnOnce() + Send + Sync>,
) -> Result<wgpu::Texture, ImageError> {
    let hal_device = unsafe { device.as_hal::<wgpu::hal::api::Metal>() }
        .ok_or(ImageError::UnsupportedBackend)?;
    let raw = unsafe { texture.cast::<RawTexture>().as_ref() };
    if Retained::as_ptr(&raw.device()) != Retained::as_ptr(hal_device.raw_device()) {
        return Err(ImageError::WrongDevice);
    }
    drop(hal_device);
    let shape = sampled_shape(raw)?;
    let retained = unsafe { retain(texture) };
    Ok(unsafe {
        texture_from_raw(
            device,
            retained,
            shape,
            Some(release),
            wgpu::wgt::TextureUses::RESOURCE,
        )
    })
}

/// What wgpu is told about an imported texture; Metal has no query for `usage`, so it is declared.
struct TextureShape {
    format: wgpu::TextureFormat,
    size: [u32; 2],
    mip_levels: u32,
    usage: wgpu::TextureUsages,
}

fn sampled_shape(raw: &RawTexture) -> Result<TextureShape, ImageError> {
    let format = match raw.pixelFormat() {
        MTLPixelFormat::RGBA8Unorm => wgpu::TextureFormat::Rgba8Unorm,
        MTLPixelFormat::BGRA8Unorm => wgpu::TextureFormat::Bgra8Unorm,
        _ => return Err(ImageError::IncompatibleTexture),
    };
    let sampleable = raw.textureType() == MTLTextureType::Type2D
        && raw.sampleCount() == 1
        && raw.arrayLength() == 1
        && raw.usage().contains(MTLTextureUsage::ShaderRead);
    if !sampleable {
        return Err(ImageError::IncompatibleTexture);
    }
    let side = |value: usize| u32::try_from(value).map_err(|_| ImageError::TooLarge);
    Ok(TextureShape {
        format,
        size: [side(raw.width())?, side(raw.height())?],
        mip_levels: u32::try_from(raw.mipmapLevelCount())
            .map_err(|_| ImageError::IncompatibleTexture)?,
        usage: wgpu::TextureUsages::TEXTURE_BINDING,
    })
}

unsafe fn retain(texture: NonNull<c_void>) -> Retained<RawTexture> {
    unsafe { Retained::retain(texture.cast::<RawTexture>().as_ptr()) }
        .expect("retaining a non-null MTLTexture")
}

unsafe fn texture_from_raw(
    device: &wgpu::Device,
    retained: Retained<RawTexture>,
    shape: TextureShape,
    release: Option<Box<dyn FnOnce() + Send + Sync>>,
    initial_state: wgpu::wgt::TextureUses,
) -> wgpu::Texture {
    let hal_texture = unsafe {
        wgpu::hal::metal::Device::texture_from_raw(
            retained,
            shape.format,
            MTLTextureType::Type2D,
            1,
            shape.mip_levels,
            wgpu::hal::CopyExtent {
                width: shape.size[0],
                height: shape.size[1],
                depth: 1,
            },
            release,
        )
    };
    let descriptor = wgpu::TextureDescriptor {
        label: Some("valo.external-metal-texture"),
        size: wgpu::Extent3d {
            width: shape.size[0],
            height: shape.size[1],
            depth_or_array_layers: 1,
        },
        mip_level_count: shape.mip_levels,
        sample_count: 1,
        dimension: wgpu::TextureDimension::D2,
        format: shape.format,
        usage: shape.usage,
        view_formats: &[],
    };
    unsafe {
        device.create_texture_from_hal::<wgpu::hal::api::Metal>(
            hal_texture,
            &descriptor,
            initial_state,
        )
    }
}
