//! Apple's system image codecs for Valo, through ImageIO and Core Graphics.
//!
//! ImageIO reads every format the OS does — HEIC and camera RAW included — with hardware help
//! where the platform has it, and can scale while it decodes. A frame is rasterised straight into
//! an `IOSurface`-backed CoreVideo buffer that Metal samples directly, so decoded pixels never
//! travel back through the CPU; when that path is unavailable the same raster lands in ordinary
//! memory and is uploaded.
//!
//! Animations are read here too. ImageIO composites them itself: every frame comes back at the
//! full canvas size with the frames before it already in place and their disposal applied, so
//! nothing here assembles a canvas.
#![cfg(any(target_os = "macos", target_os = "ios"))]
#![warn(missing_docs)]

mod raster;
mod shared;
mod source;

use source::Source;
use valo_codec::{
    DecodeError, DecodedFrame, Decoder, FramePixels, FrameReader, ImageInfo, OpenError, OpenRequest,
};

/// ImageIoDecoder reads still images with the platform's codecs.
///
/// Register it before a software decoder: it declines what it does not read, and the software
/// decoder picks those up.
#[derive(Clone, Copy, Debug)]
pub struct ImageIoDecoder {
    /// `prefer_shared` rasterises into GPU-shared memory when Metal can sample it, avoiding an
    /// upload. Turn it off to force the CPU path, for comparison or on a non-Metal device.
    pub prefer_shared: bool,
}

impl Default for ImageIoDecoder {
    fn default() -> Self {
        Self {
            prefer_shared: true,
        }
    }
}

impl Decoder for ImageIoDecoder {
    fn name(&self) -> &'static str {
        "imageio"
    }

    fn open(&self, request: &OpenRequest) -> Result<Box<dyn FrameReader>, OpenError> {
        let source = Source::open(&request.encoded)?;
        let oriented_size = source.oriented_size();
        request.options.limits.check_source_size(oriented_size)?;
        let info = ImageInfo {
            size: request.options.fit(oriented_size),
            frame_count: source.frame_count(),
            repetition: source.repetition(),
        };
        Ok(Box::new(ImageIoReader {
            source,
            info,
            next: 0,
            device: request.device.clone(),
            prefer_shared: self.prefer_shared,
        }))
    }
}

struct ImageIoReader {
    source: Source,
    info: ImageInfo,
    /// The frame the next request decodes; an animation wraps round to the first after the last.
    next: u32,
    device: wgpu::Device,
    prefer_shared: bool,
}

impl ImageIoReader {
    fn rasterize(&self, image: &core_graphics::image::CGImage) -> Result<FramePixels, DecodeError> {
        if self.prefer_shared {
            match shared::rasterize_to_metal(image, self.info.size, &self.device) {
                Ok(texture) => return Ok(FramePixels::Gpu(texture)),
                Err(shared::SharedError::Unavailable) => {}
                Err(shared::SharedError::Fatal(error)) => return Err(error),
            }
        }
        Ok(FramePixels::Cpu(raster::rasterize_to_cpu(
            image,
            self.info.size,
        )?))
    }
}

impl FrameReader for ImageIoReader {
    fn info(&self) -> ImageInfo {
        self.info
    }

    fn next_frame(&mut self) -> Result<DecodedFrame, DecodeError> {
        let index = self.next as usize;
        self.next = (self.next + 1) % self.info.frame_count.max(1);
        let longest_side = self.info.size[0].max(self.info.size[1]);
        let image = self.source.decode_scaled(index, longest_side)?;
        Ok(DecodedFrame {
            pixels: self.rasterize(&image)?,
            duration: self.source.frame_duration(index),
        })
    }
}
