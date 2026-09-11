//! The contract a codec library implements to plug into an [`ImageLoader`](crate::ImageLoader).
use crate::{DecodeError, DecodeOptions, ImageInfo};
use std::future::Future;
use std::pin::Pin;
use std::sync::Arc;
use std::time::Duration;
use valo::PixelBuffer;

/// Decoding is an answer a decoder is still working on.
///
/// Every answer here is a future because a browser hands back a promise for everything it
/// decodes; a decoder that already has its answer returns [`std::future::ready`], which costs one
/// poll. The future is made and driven on the thread that owns the decoder and never leaves it,
/// so it carries no `Send` bound.
pub type Decoding<'a, T> = Pin<Box<dyn Future<Output = T> + 'a>>;

/// Decoder is one codec library, such as Apple's ImageIO or the portable `image` crate.
///
/// A loader tries its decoders in registration order and the first one that opens the bytes
/// owns them for every frame. A decoder is a stateless registry shared across threads; the
/// [`FrameReader`] it opens is per image and stays on the thread that opened it, so a reader may
/// hold thread-affine platform objects.
pub trait Decoder: Send + Sync {
    /// `name` identifies this decoder in diagnostics when every decoder declines an image.
    fn name(&self) -> &'static str;

    /// `open` reads enough of the bytes to know the image's size and frame count.
    ///
    /// Return [`OpenError::Unsupported`] for a format or variant this decoder does not read, so
    /// the next decoder gets its turn. Anything else stops the search.
    fn open<'a>(
        &'a self,
        request: &'a OpenRequest,
    ) -> Decoding<'a, Result<Box<dyn FrameReader>, OpenError>>;
}

/// OpenRequest is everything a decoder needs to open one image.
#[derive(Clone)]
pub struct OpenRequest {
    /// `encoded` is the complete file. It is shared so a reader can keep it without copying.
    pub encoded: Arc<[u8]>,

    /// `options` bound the decode and say what size the caller wants.
    pub options: DecodeOptions,

    /// `device` is where a native decoder imports a texture it produced. Software decoders
    /// ignore it.
    pub device: wgpu::Device,
}

/// OpenError is why a decoder did not open an image.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum OpenError {
    /// Unsupported means this decoder does not read the format or variant; the next decoder is
    /// tried. The text names what was missing, for diagnostics.
    Unsupported(String),

    /// Failed means the image was recognised but cannot be decoded, or a limit was hit; no other
    /// decoder is tried.
    Failed(DecodeError),
}

impl From<DecodeError> for OpenError {
    fn from(error: DecodeError) -> Self {
        Self::Failed(error)
    }
}

/// FrameReader is one opened image, yielding complete composited frames in order.
///
/// Animation composition — disposal, blending, partial frames — is the reader's job; every frame
/// it returns is a full picture. Readers are used from one thread and dropped there.
pub trait FrameReader {
    /// `info` is the output size after [`DecodeOptions::max_size`], the frame count and how the
    /// animation repeats. It is fixed at open and every frame must match it.
    fn info(&self) -> ImageInfo;

    /// `next_frame` decodes the next frame, wrapping to the first after the last.
    ///
    /// One frame is decoded at a time: a reader is asked again only once the answer it gave has
    /// been waited for.
    fn next_frame(&mut self) -> Decoding<'_, Result<DecodedFrame, DecodeError>>;
}

/// DecodedFrame is one complete frame as a decoder produced it, before it becomes an image.
pub struct DecodedFrame {
    /// `pixels` is where the frame lives: CPU memory or a texture on the request's device.
    pub pixels: FramePixels,

    /// `duration` is how long the frame stays on screen; zero for a still image or when the file
    /// gives no delay.
    pub duration: Duration,
}

/// FramePixels is what a decoder hands back — never an image; the loader makes those.
pub enum FramePixels {
    /// Cpu is owned samples in any layout [`PixelBuffer`] accepts; the loader uploads them.
    Cpu(PixelBuffer),

    /// Gpu is a texture already holding the frame on the request's device, so nothing is copied
    /// back through the CPU. It must be 2D, single-sample, `Rgba8Unorm` or `Bgra8Unorm`, carry
    /// `TEXTURE_BINDING`, and hold premultiplied sRGB samples. Whatever backs it must stay alive
    /// until wgpu drops the texture; see `valo::import_metal_texture`.
    Gpu(wgpu::Texture),
}

impl DecodedFrame {
    /// `still` is a frame with no on-screen duration, the shape of every still image.
    pub fn still(pixels: FramePixels) -> Self {
        Self {
            pixels,
            duration: Duration::ZERO,
        }
    }
}
