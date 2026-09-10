//! Drawing a decoded `CGImage` into caller memory in the layout the renderer uploads.
use core_graphics::base::{
    kCGBitmapByteOrder32Big, kCGBitmapByteOrder32Little, kCGImageAlphaPremultipliedFirst,
    kCGImageAlphaPremultipliedLast,
};
use core_graphics::color_space::{kCGColorSpaceSRGB, CGColorSpace};
use core_graphics::context::{CGBlendMode, CGContext, CGInterpolationQuality};
use core_graphics::geometry::{CGPoint, CGRect, CGSize};
use core_graphics::image::CGImage;
use foreign_types::ForeignType;
use std::ffi::c_void;
use valo::{AlphaType, PixelBuffer, PixelFormat, PixelLayout};
use valo_codec::DecodeError;

extern "C" {
    fn CGBitmapContextCreate(
        data: *mut c_void,
        width: usize,
        height: usize,
        bits_per_component: usize,
        bytes_per_row: usize,
        space: core_graphics::sys::CGColorSpaceRef,
        bitmap_info: u32,
    ) -> core_graphics::sys::CGContextRef;
}

/// Destination is where a raster lands: caller-owned rows of premultiplied sRGB samples.
pub(crate) struct Destination {
    pub pixels: *mut u8,
    pub size: [u32; 2],
    pub row_bytes: usize,
    pub format: PixelFormat,
}

/// `rasterize_to_cpu` draws `image` into a fresh buffer of exactly `size`.
pub(crate) fn rasterize_to_cpu(
    image: &CGImage,
    size: [u32; 2],
) -> Result<PixelBuffer, DecodeError> {
    let layout = PixelLayout::packed(size, PixelFormat::Rgba8, AlphaType::Premultiplied);
    let length = layout.byte_len()?;
    let mut pixels = Vec::new();
    pixels
        .try_reserve_exact(length)
        .map_err(|_| DecodeError::Failed("out of memory for decoded pixels".into()))?;
    pixels.resize(length, 0);
    draw_into(
        image,
        Destination {
            pixels: pixels.as_mut_ptr(),
            size,
            row_bytes: layout.row_bytes as usize,
            format: PixelFormat::Rgba8,
        },
    )?;
    Ok(PixelBuffer::new(layout, pixels)?)
}

/// `draw_into` scales `image` to fill the destination in one pass.
///
/// The bitmap context is built over the caller's memory, so colour space conversion, channel
/// order and premultiplication all happen in this single draw.
///
/// The destination memory must stay valid and unaliased for the duration of the call.
pub(crate) fn draw_into(image: &CGImage, destination: Destination) -> Result<(), DecodeError> {
    let color_space = CGColorSpace::create_with_name(unsafe { kCGColorSpaceSRGB })
        .ok_or_else(|| DecodeError::Failed("sRGB color space unavailable".into()))?;
    let bitmap_info = match destination.format {
        PixelFormat::Bgra8 => kCGImageAlphaPremultipliedFirst | kCGBitmapByteOrder32Little,
        PixelFormat::Rgba8 => kCGImageAlphaPremultipliedLast | kCGBitmapByteOrder32Big,
    };
    let raw = unsafe {
        CGBitmapContextCreate(
            destination.pixels.cast(),
            destination.size[0] as usize,
            destination.size[1] as usize,
            8,
            destination.row_bytes,
            color_space.as_ptr(),
            bitmap_info,
        )
    };
    if raw.is_null() {
        return Err(DecodeError::Failed(
            "Core Graphics refused the bitmap context".into(),
        ));
    }
    let context = unsafe { CGContext::from_ptr(raw) };
    let bounds = CGRect::new(
        &CGPoint::new(0.0, 0.0),
        &CGSize::new(
            f64::from(destination.size[0]),
            f64::from(destination.size[1]),
        ),
    );
    context.set_blend_mode(CGBlendMode::Copy);
    context.set_interpolation_quality(CGInterpolationQuality::CGInterpolationQualityHigh);
    context.draw_image(bounds, image);
    Ok(())
}
