//! Image decoding for Valo: encoded bytes in, drawable [`Image`](valo::Image)s out.
//!
//! The engine itself never decodes (hosts upload images); this crate is the optional piece a host
//! adds when it wants Valo to do that job. It has no codecs of its own. A host registers
//! [`Decoder`]s — the platform's native one first where it exists, a portable software one as
//! fallback — and an [`ImageLoader`] tries them in that order for each image.
//!
//! Decoders produce pixels or a texture; the loader alone turns those into images through
//! [`ImageContext`](valo::ImageContext). That split is what lets a native codec deliver a texture
//! it already decoded into GPU-shared memory, with no readback, while software decoders never
//! touch the GPU.
//!
//! Results arrive as [`Pending`] values, which are futures. With [`ImageLoader::new`] the work
//! runs inside the first poll, on the polling thread — the shape for a single-threaded host or
//! the web. With `ImageLoader::with_worker` (the `worker` feature) a dedicated thread decodes and
//! uploads, and the poll wakes when the image is ready.
//!
//! ```no_run
//! # use std::sync::Arc;
//! # async fn demo(context: &valo::Context, decoders: Vec<Box<dyn valo_codec::Decoder>>, bytes: Arc<[u8]>) -> Result<(), valo_codec::DecodeError> {
//! use valo_codec::{DecodeOptions, ImageLoader};
//!
//! let loader = ImageLoader::new(context.image_context(), decoders);
//! let options = DecodeOptions { max_size: Some([512, 512]), mipmaps: true, ..Default::default() };
//! let image = loader.decode(bytes, options).await?;
//! # Ok(()) }
//! ```

#![warn(missing_docs)]
// A decoded frame carries a wgpu texture, and proving one `Send` walks a type graph deeper than
// the default limit once a host builds wgpu with every backend.
#![recursion_limit = "256"]

mod backend;
mod error;
mod loader;
mod options;
mod pending;
mod service;
#[cfg(all(feature = "worker", not(target_arch = "wasm32")))]
mod worker;

pub use backend::{
    DecodedFrame, Decoder, Decoding, FramePixels, FrameReader, OpenError, OpenRequest,
};
pub use error::{Declined, DecodeError};
pub use loader::{Codec, Frame, ImageLoader};
pub use options::{fit_within, DecodeLimits, DecodeOptions, ImageInfo, Repetition};
pub use pending::Pending;
