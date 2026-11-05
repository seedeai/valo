//! Display lists as PDF pages, through krilla: paths, strokes, gradients, clips, transforms,
//! opacity and blend modes stay vector, and glyph runs become real text, their fonts embedded and
//! their text kept, so it can be selected and searched. What PDF can't hold (blurs, backdrop
//! blurs, colour and image filters, masks, images and image fills) goes in as pictures valo draws,
//! as Skia's PDF backend draws them at its raster DPI.
//!
//! A page takes two passes. The first finds those parts, draws them and asks the GPU for their
//! pixels ([`draw_page`]); they come back as any of valo's pixel requests do, by the browser on
//! the web and by the host's `Context::check_requests` natively. The second writes the page with
//! the pictures in place ([`PdfDocument::add`]).

mod picture;
mod write;

use std::fmt;
use std::future::Future;
use std::pin::Pin;
use std::sync::Arc;
use std::task::{self, Poll};

use valo::{Context, DisplayList, PixelBuffer, PixelReadError, PixelRequest, Rect};

/// `PageSettings` is how a display list becomes a page.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct PageSettings {
    /// `points_per_unit` is how big a unit of the list is on the page, in PDF points.
    pub points_per_unit: f32,
    /// `pixels_per_unit` is how sharp the pictures are.
    pub pixels_per_unit: f32,
    /// `quality` is an opaque picture's, as a JPEG, from 1 to 100.
    pub quality: u8,
}

/// `PdfError` is why a page or a document couldn't be made.
#[derive(Clone, Debug, PartialEq)]
pub enum PdfError {
    /// `Read` is a picture's pixels not coming back.
    Read(PixelReadError),
    /// `Encode` is a picture that couldn't be written.
    Encode(String),
    /// `Write` is krilla refusing the document.
    Write(String),
}

impl fmt::Display for PdfError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            PdfError::Read(error) => write!(f, "{error}"),
            PdfError::Encode(why) => write!(f, "a picture couldn't be written: {why}"),
            PdfError::Write(why) => write!(f, "the PDF couldn't be written: {why}"),
        }
    }
}

impl std::error::Error for PdfError {}

/// `draw_page` draws what a page of `list`, `size` units, holds that PDF can't, asks the GPU for
/// its pixels, and returns the page, complete once they come. The drawing goes to the GPU now;
/// the future borrows nothing, so a host goes on drawing while it waits.
pub fn draw_page(
    context: &mut Context,
    list: &Arc<DisplayList>,
    size: [f32; 2],
    settings: PageSettings,
) -> PdfPageFuture {
    let sites = picture::sites(list, size, settings.pixels_per_unit);
    let requests = sites
        .iter()
        .map(|site| {
            site.as_ref()
                .map(|site| context.request_pixels(&site.list, site.pixels, None))
        })
        .collect();
    PdfPageFuture {
        page: Some(PdfPage {
            list: Arc::clone(list),
            size,
            settings,
            pictures: Vec::new(),
        }),
        sites: sites
            .into_iter()
            .map(|site| site.map(|site| site.bounds))
            .collect(),
        requests,
    }
}

/// `PdfPageFuture` is a page whose pictures the GPU is drawing. It completes as
/// `Context::check_requests` delivers them natively, or as the browser does on the web.
pub struct PdfPageFuture {
    page: Option<PdfPage>,
    sites: Vec<Option<Rect>>,
    requests: Vec<Option<PixelRequest>>,
}

impl Future for PdfPageFuture {
    type Output = Result<PdfPage, PdfError>;

    fn poll(self: Pin<&mut Self>, cx: &mut task::Context<'_>) -> Poll<Self::Output> {
        let this = self.get_mut();
        let page = this
            .page
            .as_mut()
            .expect("a finished PdfPageFuture isn't polled again");
        while page.pictures.len() < this.requests.len() {
            let at = page.pictures.len();
            let picture = match (&mut this.requests[at], this.sites[at]) {
                (Some(request), Some(bounds)) => match Pin::new(request).poll(cx) {
                    Poll::Pending => return Poll::Pending,
                    Poll::Ready(Err(error)) => return Poll::Ready(Err(PdfError::Read(error))),
                    Poll::Ready(Ok(pixels)) => Some(Picture { bounds, pixels }),
                },
                _ => None,
            };
            page.pictures.push(picture);
        }
        Poll::Ready(Ok(this.page.take().expect("the page is there until taken")))
    }
}

/// `PdfPage` is a page ready to write: its list, and the pictures of what PDF can't hold.
pub struct PdfPage {
    list: Arc<DisplayList>,
    size: [f32; 2],
    settings: PageSettings,
    // `pictures` follow the order the list holds them in; `None` for one that shows nowhere.
    pictures: Vec<Option<Picture>>,
}

/// `Picture` is part of a page as pixels, and where they go on the page, in its units.
struct Picture {
    bounds: Rect,
    pixels: PixelBuffer,
}

/// `PdfDocument` is a PDF being written, a page at a time.
pub struct PdfDocument {
    writer: write::DocumentWriter,
}

impl PdfDocument {
    pub fn new() -> Self {
        PdfDocument {
            writer: write::DocumentWriter::new(),
        }
    }

    /// `add` writes `page` as the document's next page.
    pub fn add(&mut self, page: PdfPage) -> Result<(), PdfError> {
        self.writer.page(&page)
    }

    /// `finish` is the document's file.
    pub fn finish(self) -> Result<Vec<u8>, PdfError> {
        self.writer.finish()
    }
}

impl Default for PdfDocument {
    fn default() -> Self {
        PdfDocument::new()
    }
}
