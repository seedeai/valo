//! The decode step itself: choose a decoder, read frames, make images. Reading runs wherever
//! the loader says — on the caller's thread or on a worker — and this is the only place a
//! frame becomes an image, which happens on the thread that asked: an upload is GPU work, and
//! a second thread submitting GPU work races the first's surface, which wgpu refuses.
use crate::{
    Declined, DecodeError, DecodeOptions, DecodedFrame, Decoder, Frame, FramePixels, FrameReader,
    ImageInfo, OpenError, OpenRequest,
};
use std::sync::Arc;
use std::time::Duration;
use valo::{Image, ImageContext};

/// A frame read but not yet made into an image: what a worker hands the thread that asked.
pub(crate) struct Decoded {
    pixels: FramePixels,
    /// The size the reader promised, which the image must have.
    expected: [u32; 2],
    duration: Duration,
}

/// DecodeService owns the decoder list and the image context; it is shared, stateless, and
/// `Send + Sync`, so one instance serves the caller's thread and a worker alike.
pub(crate) struct DecodeService {
    images: ImageContext,
    decoders: Vec<Box<dyn Decoder>>,
}

impl DecodeService {
    pub(crate) fn new(images: ImageContext, decoders: Vec<Box<dyn Decoder>>) -> Self {
        Self { images, decoders }
    }

    /// `open` finds the first decoder that accepts the bytes and checks what it reports.
    pub(crate) async fn open(
        &self,
        encoded: Arc<[u8]>,
        options: DecodeOptions,
    ) -> Result<Box<dyn FrameReader>, DecodeError> {
        self.check_input(&encoded, options)?;
        let request = OpenRequest {
            encoded,
            options,
            device: self.images.device().clone(),
        };
        let reader = self.first_accepting_decoder(&request).await?;
        self.check_info(reader.info(), options)?;
        Ok(reader)
    }

    /// `decode_still` opens and reads the first frame in one step, the shape of a still image.
    pub(crate) async fn decode_still(
        &self,
        encoded: Arc<[u8]>,
        options: DecodeOptions,
    ) -> Result<Image, DecodeError> {
        let decoded = self.decode_still_frame(encoded, options).await?;
        Ok(self.finish(decoded, options.mipmaps)?.image)
    }

    /// `decode_still_frame` opens and reads the first frame, leaving the image to `finish`.
    pub(crate) async fn decode_still_frame(
        &self,
        encoded: Arc<[u8]>,
        options: DecodeOptions,
    ) -> Result<Decoded, DecodeError> {
        let mut reader = self.open(encoded, options).await?;
        self.read_frame(reader.as_mut()).await
    }

    /// `next_frame` reads one frame and turns it into an image of the size the reader promised.
    pub(crate) async fn next_frame(
        &self,
        reader: &mut dyn FrameReader,
        mipmaps: bool,
    ) -> Result<Frame, DecodeError> {
        let decoded = self.read_frame(reader).await?;
        self.finish(decoded, mipmaps)
    }

    /// `read_frame` reads one frame, wherever the reader lives.
    pub(crate) async fn read_frame(
        &self,
        reader: &mut dyn FrameReader,
    ) -> Result<Decoded, DecodeError> {
        let expected = reader.info().size;
        let DecodedFrame { pixels, duration } = reader.next_frame().await?;
        Ok(Decoded {
            pixels,
            expected,
            duration,
        })
    }

    /// `finish` makes the image of a frame read, on the thread that asked for it.
    pub(crate) fn finish(&self, decoded: Decoded, mipmaps: bool) -> Result<Frame, DecodeError> {
        let Decoded {
            pixels,
            expected,
            duration,
        } = decoded;
        let image = self.make_image(pixels, mipmaps)?;
        if image.size() != expected {
            return Err(DecodeError::InvalidData(
                "frame size differs from the image header".into(),
            ));
        }
        Ok(Frame { image, duration })
    }

    fn make_image(&self, pixels: FramePixels, mipmaps: bool) -> Result<Image, DecodeError> {
        let image = match pixels {
            FramePixels::Cpu(buffer) => self.images.upload_pixels(buffer, mipmaps)?,
            FramePixels::Gpu(texture) => self.images.import_texture(texture, mipmaps)?,
        };
        Ok(image)
    }

    fn check_input(&self, encoded: &[u8], options: DecodeOptions) -> Result<(), DecodeError> {
        if self.decoders.is_empty() {
            return Err(DecodeError::NoDecoder);
        }
        if encoded.is_empty() {
            return Err(DecodeError::InvalidData("empty encoded image".into()));
        }
        if encoded.len() > options.limits.max_encoded_bytes {
            return Err(DecodeError::LimitExceeded("encoded bytes"));
        }
        Ok(())
    }

    fn check_info(&self, info: ImageInfo, options: DecodeOptions) -> Result<(), DecodeError> {
        options.limits.check_frame_count(info.frame_count)?;
        options.limits.check_source_size(info.size)?;
        self.images.check_size(info.size)?;
        Ok(())
    }

    async fn first_accepting_decoder(
        &self,
        request: &OpenRequest,
    ) -> Result<Box<dyn FrameReader>, DecodeError> {
        let mut declined = Vec::new();
        for decoder in &self.decoders {
            match decoder.open(request).await {
                Ok(reader) => return Ok(reader),
                Err(OpenError::Unsupported(reason)) => declined.push(Declined {
                    decoder: decoder.name(),
                    reason,
                }),
                Err(OpenError::Failed(error)) => return Err(error),
            }
        }
        Err(DecodeError::Unsupported(declined))
    }
}
