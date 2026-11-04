//! Encoding pixels as image files on the CPU, the way back from decoding: the same codec
//! libraries, compiled in by the same Cargo features.

use std::borrow::Cow;
use std::fmt;

use image::{ExtendedColorType, ImageEncoder};
#[cfg(feature = "jpeg")]
use valo::Color;
use valo::{AlphaType, PixelBuffer, PixelFormat};

/// `Encoding` is the file `encode` writes, with its format's settings.
#[derive(Clone, Copy, Debug, PartialEq)]
pub enum Encoding {
    /// `Png` is an RGBA PNG, transparency kept.
    #[cfg(feature = "png")]
    Png,
    /// `Jpeg` is a JPEG at `quality`, from 1 to 100, laid over `background`, since a JPEG holds
    /// no transparency; the background's own alpha is ignored.
    #[cfg(feature = "jpeg")]
    Jpeg {
        /// `quality` trades size for detail, from 1 to 100.
        quality: u8,
        /// `background` shows through where the pixels are transparent.
        background: Color,
    },
    /// `WebpLossless` is a lossless WebP, transparency kept.
    #[cfg(feature = "webp")]
    WebpLossless,
}

/// `EncodeError` is why pixels could not be encoded.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum EncodeError {
    /// `Quality` is a JPEG quality outside 1 to 100.
    Quality(u8),
    /// `Failed` is the encoder failing, with its reason.
    Failed(String),
}

impl fmt::Display for EncodeError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            EncodeError::Quality(quality) => {
                write!(f, "a JPEG's quality is 1 to 100, not {quality}")
            }
            EncodeError::Failed(why) => write!(f, "encoding failed: {why}"),
        }
    }
}

impl std::error::Error for EncodeError {}

/// `encode` writes `pixels` as a file in `encoding`. Pixels of any layout are taken, turned into
/// packed rows of straight RGBA first when they aren't already.
pub fn encode(pixels: &PixelBuffer, encoding: Encoding) -> Result<Vec<u8>, EncodeError> {
    let rgba = straight_rgba(pixels);
    let [width, height] = pixels.layout().size;
    let mut bytes = Vec::new();
    let written = match encoding {
        #[cfg(feature = "png")]
        Encoding::Png => image::codecs::png::PngEncoder::new(&mut bytes).write_image(
            &rgba,
            width,
            height,
            ExtendedColorType::Rgba8,
        ),
        #[cfg(feature = "jpeg")]
        Encoding::Jpeg {
            quality,
            background,
        } => {
            if !(1..=100).contains(&quality) {
                return Err(EncodeError::Quality(quality));
            }
            image::codecs::jpeg::JpegEncoder::new_with_quality(&mut bytes, quality).write_image(
                &over(&rgba, background),
                width,
                height,
                ExtendedColorType::Rgb8,
            )
        }
        #[cfg(feature = "webp")]
        Encoding::WebpLossless => image::codecs::webp::WebPEncoder::new_lossless(&mut bytes)
            .write_image(&rgba, width, height, ExtendedColorType::Rgba8),
    };
    written.map_err(|error| EncodeError::Failed(error.to_string()))?;
    Ok(bytes)
}

/// `straight_rgba` is `pixels` as packed rows of straight RGBA, borrowed when they already are.
fn straight_rgba(pixels: &PixelBuffer) -> Cow<'_, [u8]> {
    let layout = pixels.layout();
    let row = layout.size[0] as usize * 4;
    let packed = layout.row_bytes as usize == row;
    if packed && layout.format == PixelFormat::Rgba8 && layout.alpha == AlphaType::Straight {
        return Cow::Borrowed(pixels.bytes());
    }
    let mut rgba: Vec<u8> = pixels
        .bytes()
        .chunks(layout.row_bytes as usize)
        .take(layout.size[1] as usize)
        .flat_map(|samples| &samples[..row])
        .copied()
        .collect();
    if layout.format == PixelFormat::Bgra8 {
        for pixel in rgba.as_chunks_mut::<4>().0 {
            pixel.swap(0, 2);
        }
    }
    if layout.alpha == AlphaType::Premultiplied {
        valo::unpremultiply(&mut rgba);
    }
    Cow::Owned(rgba)
}

/// `over` is straight RGBA laid over an opaque `background`, as RGB.
#[cfg(feature = "jpeg")]
fn over(rgba: &[u8], background: Color) -> Vec<u8> {
    let below = [background.r, background.g, background.b]
        .map(|channel| (channel.clamp(0.0, 1.0) * 255.0).round() as u16);
    rgba.as_chunks::<4>()
        .0
        .iter()
        .flat_map(|&[red, green, blue, alpha]| {
            let alpha = u16::from(alpha);
            let mut rgb = [red, green, blue];
            for (channel, below) in rgb.iter_mut().zip(below) {
                *channel =
                    ((u16::from(*channel) * alpha + below * (255 - alpha) + 127) / 255) as u8;
            }
            rgb
        })
        .collect()
}
