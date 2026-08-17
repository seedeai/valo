//! SVG parsing and translation into Valo display lists.
//!
//! Translation is best-effort. Unsupported features degrade only the affected
//! element and are reported through [`Svg::missing`]; they do not reject the
//! complete document.

use std::collections::HashMap;
use std::io::Read;
use std::sync::Arc;

use valo_dl::DisplayList;

mod convert;
mod translate;

/// `Svg` is a translated vector document ready to record or draw.
///
/// Its display list uses the document's intrinsic coordinate space. Apply a
/// transform when drawing to fit it into another destination size.
pub struct Svg {
    /// `list` contains the translated drawing commands.
    pub list: Arc<DisplayList>,
    /// `size` is the resolved intrinsic width and height in CSS pixels.
    pub size: [f32; 2],
    /// `missing` lists unsupported feature tags in first-seen order without duplicates.
    ///
    /// Affected elements may be simplified or omitted.
    pub missing: Vec<&'static str>,
}

/// `Document` is a parsed SVG retained for resolving embedded images before translation.
///
/// Call [`Self::images`] to discover raster images, decode and upload them
/// through the host, then provide those image handles to [`Self::translate`].
pub struct Document {
    tree: usvg::Tree,
    has_text: bool,
    images: Vec<ImageData>,
    /// ImageKind bytes-pointer → request id (identity dedupes shared refs).
    ids: HashMap<usize, u32>,
}

/// `ImageData` describes one deduplicated raster image embedded in an SVG.
#[derive(Clone)]
pub struct ImageData {
    /// `id` identifies the image when resolving [`Document::translate`].
    pub id: u32,
    /// `format` identifies the encoded image format.
    pub format: ImageFormat,
    /// `bytes` contains the original encoded image data.
    pub bytes: Arc<Vec<u8>>,
}

/// `ImageFormat` identifies a supported embedded raster encoding.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ImageFormat {
    /// `Png` is Portable Network Graphics.
    Png,
    /// `Jpeg` is Joint Photographic Experts Group encoding.
    Jpeg,
    /// `Gif` is Graphics Interchange Format.
    Gif,
    /// `Webp` is WebP encoding.
    Webp,
}

impl ImageFormat {
    /// `as_str` returns the lowercase format name.
    pub fn as_str(self) -> &'static str {
        match self {
            ImageFormat::Png => "png",
            ImageFormat::Jpeg => "jpeg",
            ImageFormat::Gif => "gif",
            ImageFormat::Webp => "webp",
        }
    }
}

/// `SvgError` reports failures that prevent any document from being translated.
///
/// Unsupported SVG features are not errors; they are reported through
/// [`Svg::missing`].
#[derive(Debug)]
pub enum SvgError {
    /// `Parse` indicates invalid SVG, invalid UTF-8/XML, or malformed gzip data.
    Parse,
}

/// `parse` reads plain SVG or gzip-compressed SVGZ bytes into a [`Document`].
pub fn parse(svg: &[u8]) -> Result<Document, SvgError> {
    let decompressed;
    let svg = if svg.starts_with(&[0x1f, 0x8b]) {
        decompressed = gunzip(svg)?;
        &decompressed
    } else {
        svg
    };
    // Without usvg's `text` feature the parser silently DROPS <text>
    // elements — detect on the raw bytes so the gap is at least reported
    // (false positives only cost a spurious tag).
    let has_text = contains_text_element(svg);
    let options = usvg::Options::default();
    let tree = usvg::Tree::from_data(svg, &options).map_err(|_| SvgError::Parse)?;
    let (images, ids) = collect_images(&tree);
    Ok(Document {
        tree,
        has_text,
        images,
        ids,
    })
}

/// `translate` parses and translates SVG bytes without resolving embedded images.
///
/// Use it when the document has no embedded rasters. Embedded images are
/// omitted and reported as `"image"` in [`Svg::missing`].
pub fn translate(svg: &[u8]) -> Result<Svg, SvgError> {
    Ok(parse(svg)?.translate(&|_| None))
}

impl Document {
    /// `images` returns deduplicated embedded rasters for host decoding and upload.
    pub fn images(&self) -> &[ImageData] {
        &self.images
    }

    /// `size` returns the resolved intrinsic width and height in CSS pixels.
    pub fn size(&self) -> [f32; 2] {
        [self.tree.size().width(), self.tree.size().height()]
    }

    /// `translate` records the parsed document into a Valo display list.
    ///
    /// `resolve` maps each [`ImageData::id`] to a host-uploaded image. Returning
    /// `None` omits that raster element and adds `"image"` to [`Svg::missing`].
    pub fn translate(&self, resolve: &dyn Fn(u32) -> Option<valo_dl::Image>) -> Svg {
        let mut missing = translate::Missing::default();
        if self.has_text {
            missing.add("text");
        }
        let list = translate::root(&self.tree, &mut missing, &self.ids, resolve);
        Svg {
            list: Arc::new(list),
            size: self.size(),
            missing: missing.into_tags(),
        }
    }
}

#[cfg(feature = "decode")]
impl ImageData {
    /// `decode` software-decodes the image into straight-alpha RGBA8 pixels.
    ///
    /// It returns `(width, height, pixels)` with tightly packed rows, or `None`
    /// when decoding fails. Prefer a host hardware decoder when available.
    pub fn decode(&self) -> Option<(u32, u32, Vec<u8>)> {
        let decoded = image::load_from_memory(&self.bytes).ok()?;
        let rgba = decoded.to_rgba8();
        Some((rgba.width(), rgba.height(), rgba.into_raw()))
    }
}

/// Every raster `ImageKind` in the tree (all subroots — masks, clips,
/// patterns included), keyed by bytes identity so shared refs request once.
fn collect_images(tree: &usvg::Tree) -> (Vec<ImageData>, HashMap<usize, u32>) {
    let mut images = Vec::new();
    let mut ids = HashMap::new();
    walk_images(tree.root(), &mut images, &mut ids);
    (images, ids)
}

fn walk_images(group: &usvg::Group, images: &mut Vec<ImageData>, ids: &mut HashMap<usize, u32>) {
    for node in group.children() {
        if let usvg::Node::Image(image) = node {
            note_image(image, images, ids);
        }
        if let usvg::Node::Group(inner) = node {
            walk_images(inner, images, ids);
        }
        node.subroots(|subroot| walk_images(subroot, images, ids));
    }
}

fn note_image(image: &usvg::Image, images: &mut Vec<ImageData>, ids: &mut HashMap<usize, u32>) {
    let (format, bytes) = match image.kind() {
        usvg::ImageKind::PNG(b) => (ImageFormat::Png, b),
        usvg::ImageKind::JPEG(b) => (ImageFormat::Jpeg, b),
        usvg::ImageKind::GIF(b) => (ImageFormat::Gif, b),
        usvg::ImageKind::WEBP(b) => (ImageFormat::Webp, b),
        // Nested svg trees translate recursively — nothing to decode.
        usvg::ImageKind::SVG(tree) => {
            walk_images(tree.root(), images, ids);
            return;
        }
    };
    let key = Arc::as_ptr(bytes) as usize;
    if ids.contains_key(&key) {
        return;
    }
    let id = images.len() as u32;
    ids.insert(key, id);
    images.push(ImageData {
        id,
        format,
        bytes: bytes.clone(),
    });
}

fn gunzip(bytes: &[u8]) -> Result<Vec<u8>, SvgError> {
    let mut out = Vec::new();
    flate2::read::GzDecoder::new(bytes)
        .read_to_end(&mut out)
        .map_err(|_| SvgError::Parse)?;
    Ok(out)
}

fn contains_text_element(svg: &[u8]) -> bool {
    svg.windows(5).any(|w| w == b"<text")
}
