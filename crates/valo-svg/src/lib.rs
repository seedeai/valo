//! SVG → valo display lists: `usvg` normalizes the document
//! (XML, CSS cascade, `defs`/`use`/markers, units) into groups and paths
//! with resolved paints; the translator WALKS that tree into recorded valo
//! ops. Skia's `modules/svg` split, with usvg standing in for the DOM.
//!
//! Rendering is BEST-EFFORT toward the full spec — flutter_svg's shape:
//! features valo can't express yet degrade PER ELEMENT (a filtered group
//! renders unfiltered, an embedded image is dropped) and surface as tags
//! in [`Svg::missing`] for the host to log. There is no whole-document
//! abort and no fallback path; the tags tell us which feature real
//! documents demand next.

use std::collections::HashMap;
use std::io::Read;
use std::sync::Arc;

use valo_dl::DisplayList;

mod convert;
mod translate;

/// A translated SVG document: draw `list` fitted from `size` (the viewBox
/// units) into the destination — crisp at every zoom.
pub struct Svg {
    pub list: Arc<DisplayList>,
    pub size: [f32; 2],
    /// Deduped tags for features the document uses but the translator
    /// cannot express yet — those elements rendered without them.
    pub missing: Vec<&'static str>,
}

/// A parsed document, retained between [`parse`] and [`Document::translate`]
/// so the host can decode its embedded rasters first: fetch
/// [`Document::images`], decode each (hardware where available), then
/// translate with a resolver mapping ids to uploaded textures.
pub struct Document {
    tree: usvg::Tree,
    has_text: bool,
    images: Vec<ImageData>,
    /// ImageKind bytes-pointer → request id (identity dedupes shared refs).
    ids: HashMap<usize, u32>,
}

/// One embedded raster the host should decode.
#[derive(Clone)]
pub struct ImageData {
    pub id: u32,
    pub format: ImageFormat,
    pub bytes: Arc<Vec<u8>>,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ImageFormat {
    Png,
    Jpeg,
    Gif,
    Webp,
}

impl ImageFormat {
    pub fn as_str(self) -> &'static str {
        match self {
            ImageFormat::Png => "png",
            ImageFormat::Jpeg => "jpeg",
            ImageFormat::Gif => "gif",
            ImageFormat::Webp => "webp",
        }
    }
}

/// Nothing rendered at all (feature gaps are NOT errors — they degrade
/// per element and report through [`Svg::missing`]).
#[derive(Debug)]
pub enum SvgError {
    /// Not parseable as SVG.
    Parse,
}

/// Parse `svg` bytes (plain or gzip-compressed svgz) into a retained
/// [`Document`].
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

/// One-shot convenience for documents without embedded rasters (any that
/// ARE embedded render absent and tag `image`).
pub fn translate(svg: &[u8]) -> Result<Svg, SvgError> {
    Ok(parse(svg)?.translate(&|_| None))
}

impl Document {
    /// Embedded rasters awaiting the host's decode, identity-deduped.
    pub fn images(&self) -> &[ImageData] {
        &self.images
    }

    pub fn size(&self) -> [f32; 2] {
        [self.tree.size().width(), self.tree.size().height()]
    }

    /// Walk the tree into valo ops; `resolve` maps [`ImageData::id`]s to
    /// uploaded textures (None renders that element absent + tags).
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
    /// Software decode to straight RGBA8 — for hosts without a hardware
    /// decoder (server export). Browsers should prefer createImageBitmap.
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
