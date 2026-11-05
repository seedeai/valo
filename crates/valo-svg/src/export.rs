//! Display lists as SVG documents: shapes, gradients, clips, transforms, opacity and blend modes
//! as SVG's own, glyphs defined once (outlines, or colour glyphs as their images), and shadows,
//! blurs, masks and colour filters as SVG's filters and masks where [`NativeEffects`] lets them.
//! What SVG can't hold, or what `NativeEffects` leaves out, goes in as pictures valo draws:
//! images, sweep gradients, other filters.
//!
//! [`write`] takes two passes, as a PDF page does. The first finds those parts, draws them and
//! asks the GPU for their pixels; they come back as any of valo's pixel requests do, by the
//! browser on the web and by the host's `Context::check_requests` natively. The second writes the
//! document with the pictures in place.

mod markup;
mod native;
mod raster;
mod writer;

use std::fmt;
use std::future::Future;
use std::pin::Pin;
use std::sync::Arc;
use std::task::{self, Poll};

use valo::{Color, Context, DisplayList, PixelBuffer, PixelReadError, PixelRequest, Rect};
use valo_codec_software::{encode, Encoding};

/// `SvgOptions` is how a display list becomes an SVG document.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct SvgOptions {
    /// `pixels_per_unit` is how sharp the pictures and colour glyphs are.
    pub pixels_per_unit: f32,
    /// `quality` is an opaque picture's, as a JPEG, from 1 to 100.
    pub quality: u8,
    /// `native` is which effects SVG draws itself; the rest become pictures.
    pub native: NativeEffects,
}

/// `NativeEffects` is which effects a document draws with SVG's own filters and masks. Each one
/// off is drawn as pictures instead, for an app that doesn't draw it.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct NativeEffects {
    /// `shadows` are a shape's (its coverage blurred) and a group's or a text's (its content in
    /// one colour, blurred).
    pub shadows: bool,
    /// `layer_blurs` blur what a layer or a draw holds.
    pub layer_blurs: bool,
    /// `masks` are the masks SVG assets bring.
    pub masks: bool,
    /// `color_filters` recolour a layer or a draw, by a colour matrix or one colour.
    pub color_filters: bool,
    /// `backdrop_blurs` blur what is under a layer, as CSS's `backdrop-filter`.
    pub backdrop_blurs: bool,
}

impl Default for NativeEffects {
    /// `default` draws every effect natively but backdrop blurs, which few apps draw in an SVG.
    fn default() -> Self {
        NativeEffects {
            shadows: true,
            layer_blurs: true,
            masks: true,
            color_filters: true,
            backdrop_blurs: false,
        }
    }
}

/// `SvgWriteError` is why a document couldn't be written.
#[derive(Clone, Debug, PartialEq)]
pub enum SvgWriteError {
    /// `Read` is a picture's pixels not coming back.
    Read(PixelReadError),
    /// `Encode` is a picture or a colour glyph that couldn't be written as an image.
    Encode(String),
}

impl fmt::Display for SvgWriteError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            SvgWriteError::Read(error) => write!(f, "{error}"),
            SvgWriteError::Encode(why) => write!(f, "an image couldn't be written: {why}"),
        }
    }
}

impl std::error::Error for SvgWriteError {}

/// `write` writes `list` as an SVG document of `size` units, a unit to a pixel of the SVG's. It
/// draws what the document won't hold and asks the GPU for its pixels now; the future, which
/// borrows nothing, gives the document once they come, so a host goes on drawing while it waits.
pub fn write(
    context: &mut Context,
    list: &Arc<DisplayList>,
    size: [f32; 2],
    options: SvgOptions,
) -> SvgFuture {
    let regions = raster::raster_regions(list, size, options.pixels_per_unit, options.native);
    let requests = regions
        .iter()
        .map(|region| {
            region
                .as_ref()
                .map(|region| context.request_pixels(&region.list, region.pixel_size, None))
        })
        .collect();
    SvgFuture {
        list: Arc::clone(list),
        size,
        options,
        bounds: regions
            .into_iter()
            .map(|region| region.map(|region| region.bounds))
            .collect(),
        requests,
        pictures: Vec::new(),
    }
}

/// `SvgFuture` is an SVG document whose pictures the GPU is drawing. It completes as
/// `Context::check_requests` delivers them natively, or as the browser does on the web.
pub struct SvgFuture {
    list: Arc<DisplayList>,
    size: [f32; 2],
    options: SvgOptions,
    bounds: Vec<Option<Rect>>,
    requests: Vec<Option<PixelRequest>>,
    // `pictures` follow the order a walk meets them in; `None` for one that shows nowhere.
    pictures: Vec<Option<Picture>>,
}

impl Future for SvgFuture {
    type Output = Result<String, SvgWriteError>;

    fn poll(self: Pin<&mut Self>, cx: &mut task::Context<'_>) -> Poll<Self::Output> {
        let this = self.get_mut();
        while this.pictures.len() < this.requests.len() {
            let at = this.pictures.len();
            let picture = match (&mut this.requests[at], this.bounds[at]) {
                (Some(request), Some(bounds)) => match Pin::new(request).poll(cx) {
                    Poll::Pending => return Poll::Pending,
                    Poll::Ready(Err(error)) => return Poll::Ready(Err(SvgWriteError::Read(error))),
                    Poll::Ready(Ok(pixels)) => match href_of(&pixels, this.options.quality) {
                        Ok(href) => Some(Picture { bounds, href }),
                        Err(error) => return Poll::Ready(Err(error)),
                    },
                },
                _ => None,
            };
            this.pictures.push(picture);
        }
        Poll::Ready(writer::document(
            &this.list,
            this.size,
            this.options,
            &this.pictures,
        ))
    }
}

/// `Picture` is part of a document as an encoded image, and where it goes, in its units.
struct Picture {
    bounds: Rect,
    href: String,
}

/// `href_of` is a picture as a `data:` URL: a JPEG when every pixel is opaque, else a PNG with
/// its transparency.
fn href_of(pixels: &PixelBuffer, quality: u8) -> Result<String, SvgWriteError> {
    let opaque = pixels
        .bytes()
        .as_chunks::<4>()
        .0
        .iter()
        .all(|pixel| pixel[3] == 255);
    if opaque {
        let background = Color::WHITE;
        encoded_href(
            pixels,
            Encoding::Jpeg {
                quality,
                background,
            },
            "image/jpeg",
        )
    } else {
        encoded_href(pixels, Encoding::Png, "image/png")
    }
}

/// `encoded_href` is `pixels` encoded as `encoding`, of the media type `kind`, as a `data:` URL.
fn encoded_href(
    pixels: &PixelBuffer,
    encoding: Encoding,
    kind: &str,
) -> Result<String, SvgWriteError> {
    let bytes =
        encode(pixels, encoding).map_err(|error| SvgWriteError::Encode(error.to_string()))?;
    Ok(markup::data_url(kind, &bytes))
}
