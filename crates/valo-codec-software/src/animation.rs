//! Composited animation frames, one at a time, from the retained encoded bytes.
use crate::decode_error;
use image::{AnimationDecoder, ImageDecoder, ImageFormat, ImageResult, RgbaImage};
use std::io::Cursor;
use std::sync::Arc;
use std::time::Duration;
use valo_codec::DecodeError;

type Frames = Box<dyn Iterator<Item = ImageResult<image::Frame>>>;

/// FrameStream walks an animation forward; a request for an earlier frame reopens the file.
///
/// The image crate's frame iterators do the compositing (disposal, blending, partial frames) and
/// hold only the current canvas, so memory stays at one frame however long the animation is.
pub(crate) struct FrameStream {
    encoded: Arc<[u8]>,
    format: ImageFormat,
    limits: image::Limits,
    frames: Option<Frames>,
    /// Index of the frame the iterator will yield next.
    next: u32,
}

impl FrameStream {
    pub(crate) fn new(encoded: Arc<[u8]>, format: ImageFormat, limits: image::Limits) -> Self {
        Self {
            encoded,
            format,
            limits,
            frames: None,
            next: 0,
        }
    }

    /// `frame` is composited frame `index` and how long the file shows it.
    pub(crate) fn frame(&mut self, index: u32) -> Result<(RgbaImage, Duration), DecodeError> {
        if self.frames.is_none() || index < self.next {
            self.frames = Some(self.open()?);
            self.next = 0;
        }
        loop {
            let frame = self.pull()?;
            let current = self.next;
            self.next += 1;
            if current == index {
                let duration = delay_of(&frame);
                return Ok((frame.into_buffer(), duration));
            }
        }
    }

    fn pull(&mut self) -> Result<image::Frame, DecodeError> {
        self.frames
            .as_mut()
            .expect("frame iterator is open")
            .next()
            .ok_or_else(|| {
                DecodeError::InvalidData("animation ended before its declared frame count".into())
            })?
            .map_err(decode_error)
    }

    fn open(&self) -> Result<Frames, DecodeError> {
        let cursor = Cursor::new(self.encoded.clone());
        match self.format {
            #[cfg(feature = "gif")]
            ImageFormat::Gif => {
                let mut decoder =
                    image::codecs::gif::GifDecoder::new(cursor).map_err(decode_error)?;
                decoder
                    .set_limits(self.limits.clone())
                    .map_err(decode_error)?;
                Ok(Box::new(decoder.into_frames()))
            }
            #[cfg(feature = "png")]
            ImageFormat::Png => {
                let mut decoder =
                    image::codecs::png::PngDecoder::new(cursor).map_err(decode_error)?;
                decoder
                    .set_limits(self.limits.clone())
                    .map_err(decode_error)?;
                Ok(Box::new(
                    decoder.apng().map_err(decode_error)?.into_frames(),
                ))
            }
            #[cfg(feature = "webp")]
            ImageFormat::WebP => {
                let mut decoder =
                    image::codecs::webp::WebPDecoder::new(cursor).map_err(decode_error)?;
                decoder
                    .set_limits(self.limits.clone())
                    .map_err(decode_error)?;
                Ok(Box::new(decoder.into_frames()))
            }
            other => Err(DecodeError::Failed(format!(
                "{other:?} has no animation support in this build"
            ))),
        }
    }
}

fn delay_of(frame: &image::Frame) -> Duration {
    let (numerator, denominator) = frame.delay().numer_denom_ms();
    Duration::from_micros(u64::from(numerator) * 1000 / u64::from(denominator.max(1)))
}
