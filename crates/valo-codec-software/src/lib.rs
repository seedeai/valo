//! Portable image decoding on the CPU, the fallback behind any native decoder.
//!
//! Formats are compiled in by Cargo feature (`png`, `jpeg`, `gif`, `webp`, `bmp`). Every frame
//! comes out oriented, resized to the request's `max_size`, and converted to 8-bit sRGB: embedded
//! ICC profiles, PNG gamma/chromaticity and CICP metadata are all applied rather than ignored.
//! Animations are composited frame by frame; only the current composition is held in memory.

#![warn(missing_docs)]

#[cfg(animation)]
mod animation;
mod color;
mod probe;

#[cfg(animation)]
use animation::FrameStream;
use color::ColorSpace;
use image::DynamicImage;
use probe::Probe;
use std::io::Cursor;
use std::sync::Arc;
use std::time::Duration;
use valo::{AlphaType, PixelBuffer, PixelFormat, PixelLayout};
use valo_codec::{
    DecodeError, DecodedFrame, Decoder, FramePixels, FrameReader, ImageInfo, OpenError, OpenRequest,
};

/// SoftwareDecoder reads the formats this crate was built with, entirely on the CPU.
///
/// Register it after a platform decoder so it only sees what the platform declined, or alone on
/// platforms without one.
#[derive(Clone, Copy, Debug, Default)]
pub struct SoftwareDecoder;

impl Decoder for SoftwareDecoder {
    fn name(&self) -> &'static str {
        "software"
    }

    fn open(&self, request: &OpenRequest) -> Result<Box<dyn FrameReader>, OpenError> {
        Ok(Box::new(SoftwareReader::open(request)?))
    }
}

/// One opened image. Still images are re-decoded from the retained bytes on every frame request
/// (they have one frame); animations keep the compositing iterator between frames.
struct SoftwareReader {
    encoded: Arc<[u8]>,
    probe: Probe,
    color: ColorSpace,
    limits: image::Limits,
    output_size: [u32; 2],
    #[cfg(animation)]
    animation: Option<FrameStream>,
    next: u32,
}

impl SoftwareReader {
    fn open(request: &OpenRequest) -> Result<Self, OpenError> {
        let limits = request.options.limits;
        let probe = probe::probe(&request.encoded, limits)?;
        let color = ColorSpace::detect(&probe, &request.encoded)?;
        let image_limits = image_limits(limits);
        #[cfg(animation)]
        let animation = probe
            .animated
            .then(|| FrameStream::new(request.encoded.clone(), probe.format, image_limits.clone()));
        Ok(Self {
            output_size: request.options.fit(probe.oriented_size),
            encoded: request.encoded.clone(),
            probe,
            color,
            limits: image_limits,
            #[cfg(animation)]
            animation,
            next: 0,
        })
    }

    fn decode_still(&self) -> Result<DynamicImage, DecodeError> {
        let mut reader =
            image::ImageReader::with_format(Cursor::new(self.encoded.clone()), self.probe.format);
        reader.limits(self.limits.clone());
        reader.decode().map_err(decode_error)
    }

    #[cfg(animation)]
    fn decoded(&mut self, index: u32) -> Result<(DynamicImage, Duration), DecodeError> {
        match &mut self.animation {
            Some(stream) => {
                let (frame, duration) = stream.frame(index)?;
                Ok((DynamicImage::ImageRgba8(frame), duration))
            }
            None => Ok((self.decode_still()?, Duration::ZERO)),
        }
    }

    #[cfg(not(animation))]
    fn decoded(&mut self, _index: u32) -> Result<(DynamicImage, Duration), DecodeError> {
        Ok((self.decode_still()?, Duration::ZERO))
    }

    fn finish(&self, mut image: DynamicImage) -> Result<PixelBuffer, DecodeError> {
        image.apply_orientation(self.probe.orientation);
        if [image.width(), image.height()] != self.output_size {
            let [width, height] = self.output_size;
            image = image.resize_exact(width, height, image::imageops::FilterType::Triangle);
        }
        let rgba = self.color.to_srgb_rgba8(image)?;
        let layout = PixelLayout::packed(self.output_size, PixelFormat::Rgba8, AlphaType::Straight);
        Ok(PixelBuffer::new(layout, rgba)?)
    }
}

impl FrameReader for SoftwareReader {
    fn info(&self) -> ImageInfo {
        ImageInfo {
            size: self.output_size,
            frame_count: self.probe.frame_count,
            repetition: self.probe.repetition,
        }
    }

    fn next_frame(&mut self) -> Result<DecodedFrame, DecodeError> {
        let index = self.next;
        self.next = (index + 1) % self.probe.frame_count;
        let (image, duration) = self.decoded(index)?;
        Ok(DecodedFrame {
            pixels: FramePixels::Cpu(self.finish(image)?),
            duration,
        })
    }
}

/// The image crate's own allocation guard, sized from ours: a frame at `max_pixels` in the
/// widest intermediate the crate uses (16 bytes a pixel) must still be allowed through.
fn image_limits(limits: valo_codec::DecodeLimits) -> image::Limits {
    let mut image_limits = image::Limits::default();
    image_limits.max_alloc = Some(limits.max_pixels.saturating_mul(16));
    image_limits
}

/// Maps the image crate's errors onto ours once a format has been accepted.
pub(crate) fn decode_error(error: image::ImageError) -> DecodeError {
    use image::ImageError;
    match error {
        ImageError::Decoding(error) => DecodeError::InvalidData(error.to_string()),
        ImageError::Limits(_) => DecodeError::LimitExceeded("software decoder allocation"),
        ImageError::Unsupported(error) => DecodeError::Failed(error.to_string()),
        other => DecodeError::Failed(other.to_string()),
    }
}
