//! The host-facing API: a loader that decodes bytes, and the codec it opens for animations.
use crate::service::DecodeService;
use crate::{DecodeOptions, Decoder, FrameReader, ImageInfo, Pending};
use std::sync::Arc;
use std::time::Duration;
use valo::{Image, ImageContext};

/// ImageLoader decodes encoded images into drawable [`Image`]s with registered decoders.
///
/// Decoders are tried in the order given; put the platform's native decoder first and a software
/// one after it. The loader is cheap to clone and every clone shares the same decoders and, with
/// `with_worker` (the `worker` feature), the same worker thread.
#[derive(Clone)]
pub struct ImageLoader {
    service: Arc<DecodeService>,
    runner: Runner,
}

#[derive(Clone)]
enum Runner {
    Inline,
    #[cfg(all(feature = "worker", not(target_arch = "wasm32")))]
    Worker(crate::worker::Worker),
}

impl ImageLoader {
    /// `new` decodes on the polling thread: each [`Pending`] does its work inside its first poll.
    ///
    /// This is the shape for hosts without threads, such as the web, and for hosts whose executor
    /// already runs on a background thread. Nothing is spawned.
    pub fn new(images: ImageContext, decoders: Vec<Box<dyn Decoder>>) -> Self {
        Self {
            service: Arc::new(DecodeService::new(images, decoders)),
            runner: Runner::Inline,
        }
    }

    /// `with_worker` decodes and uploads on one dedicated thread, waking the poller when done.
    ///
    /// Requests run in order on that thread. Opened codecs live there too, so a decoder may hold
    /// thread-affine state. The thread exits when the loader and every codec it opened are gone.
    #[cfg(all(feature = "worker", not(target_arch = "wasm32")))]
    pub fn with_worker(
        images: ImageContext,
        decoders: Vec<Box<dyn Decoder>>,
    ) -> std::io::Result<Self> {
        let service = Arc::new(DecodeService::new(images, decoders));
        let worker = crate::worker::Worker::spawn(service.clone())?;
        Ok(Self {
            service,
            runner: Runner::Worker(worker),
        })
    }

    /// `decode` produces the first frame of `encoded` as an image: the still-image path.
    pub fn decode(&self, encoded: Arc<[u8]>, options: DecodeOptions) -> Pending<'static, Image> {
        match &self.runner {
            Runner::Inline => {
                let service = self.service.clone();
                Pending::from_future(async move { service.decode_still(encoded, options).await })
            }
            #[cfg(all(feature = "worker", not(target_arch = "wasm32")))]
            Runner::Worker(worker) => worker.decode(encoded, options),
        }
    }

    /// `open` reads the image's header and returns a [`Codec`] that decodes frames on demand.
    ///
    /// Use it when frame count or repetition matter — animations — or to defer the first decode.
    pub fn open(&self, encoded: Arc<[u8]>, options: DecodeOptions) -> Pending<'static, Codec> {
        match &self.runner {
            Runner::Inline => {
                let service = self.service.clone();
                Pending::from_future(async move {
                    let reader = service.open(encoded, options).await?;
                    Ok(Codec::inline(service, reader, options.mipmaps))
                })
            }
            #[cfg(all(feature = "worker", not(target_arch = "wasm32")))]
            Runner::Worker(worker) => worker.open(encoded, options),
        }
    }
}

/// Codec is an opened image whose frames are decoded one at a time, on request.
///
/// It is the shape of Flutter's `ui.Codec`: a still image is a codec with one frame, so a caller
/// need not know in advance which it holds. Dropping the codec releases the decoder's state.
pub struct Codec {
    info: ImageInfo,
    mipmaps: bool,
    reader: Reader,
}

enum Reader {
    Inline {
        service: Arc<DecodeService>,
        reader: Box<dyn FrameReader>,
    },
    #[cfg(all(feature = "worker", not(target_arch = "wasm32")))]
    Remote(crate::worker::RemoteReader),
}

impl Codec {
    fn inline(service: Arc<DecodeService>, reader: Box<dyn FrameReader>, mipmaps: bool) -> Self {
        Self {
            info: reader.info(),
            mipmaps,
            reader: Reader::Inline { service, reader },
        }
    }

    #[cfg(all(feature = "worker", not(target_arch = "wasm32")))]
    pub(crate) fn remote(
        info: ImageInfo,
        mipmaps: bool,
        reader: crate::worker::RemoteReader,
    ) -> Self {
        Self {
            info,
            mipmaps,
            reader: Reader::Remote(reader),
        }
    }

    /// `info` is the frame size, frame count and repetition fixed when the image was opened.
    pub fn info(&self) -> ImageInfo {
        self.info
    }

    /// `next_frame` decodes the next frame, wrapping to the first after the last.
    ///
    /// The frame borrows the codec until it arrives, so one codec decodes one frame at a time and
    /// a second request cannot be made while the first is in flight.
    pub fn next_frame(&mut self) -> Pending<'_, Frame> {
        let mipmaps = self.mipmaps;
        match &mut self.reader {
            Reader::Inline { service, reader } => {
                Pending::from_future(service.next_frame(reader.as_mut(), mipmaps))
            }
            #[cfg(all(feature = "worker", not(target_arch = "wasm32")))]
            Reader::Remote(remote) => remote.next_frame(),
        }
    }
}

impl std::fmt::Debug for Codec {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Codec").field("info", &self.info).finish()
    }
}

/// Frame is one decoded frame, ready to draw.
#[derive(Clone, Debug)]
pub struct Frame {
    /// `image` is the frame, at the size the codec's [`ImageInfo`] promised.
    pub image: Image,

    /// `duration` is how long the frame stays on screen; zero for a still image or when the file
    /// gives no delay.
    pub duration: Duration,
}
