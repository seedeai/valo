//! The decode step itself: choose a decoder, read frames, make images. Runs wherever the loader
//! says — on the caller's thread or on a worker — and is the only place a frame becomes an image.
use crate::{
    Declined, DecodeError, DecodeOptions, DecodedFrame, Decoder, Frame, FramePixels, FrameReader,
    ImageInfo, OpenError, OpenRequest,
};
use std::sync::Arc;
use valo::{Image, ImageContext};

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
        let mut reader = self.open(encoded, options).await?;
        Ok(self
            .next_frame(reader.as_mut(), options.mipmaps)
            .await?
            .image)
    }

    /// `next_frame` reads one frame and turns it into an image of the size the reader promised.
    pub(crate) async fn next_frame(
        &self,
        reader: &mut dyn FrameReader,
        mipmaps: bool,
    ) -> Result<Frame, DecodeError> {
        let expected = reader.info().size;
        let DecodedFrame { pixels, duration } = reader.next_frame().await?;
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
