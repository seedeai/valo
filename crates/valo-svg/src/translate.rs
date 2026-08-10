//! The tree walk: usvg's normalized groups and paths → recorded valo ops.
//! One function per structural concern (group scope, clips, masks, path
//! draws, pattern tiling); leaf type mapping lives in [`crate::convert`].
//!
//! Degrade policy (flutter_svg's): a feature gap skips the SMALLEST thing
//! it can — one paint, one element, one effect — records its tag in
//! [`Missing`], and the rest of the document still renders.

use std::collections::HashMap;
use std::sync::Arc;

use valo_dl::{ClipOp, DisplayList, DisplayListBuilder, MaskKind, Paint, Sampling};
use valo_geometry::{Color, FillRule, Path, Rect};

use crate::convert;

/// One translation pass: the missing-feature ledger plus the embedded-
/// image resolver (ids assigned at parse, textures supplied
/// by the host).
pub(crate) struct Ctx<'a> {
    pub missing: &'a mut Missing,
    ids: &'a HashMap<usize, u32>,
    resolve: &'a dyn Fn(u32) -> Option<valo_dl::Image>,
}

impl Ctx<'_> {
    fn add(&mut self, tag: &'static str) {
        self.missing.add(tag);
    }

    fn image_for(&self, bytes: &Arc<Vec<u8>>) -> Option<valo_dl::Image> {
        let id = *self.ids.get(&(Arc::as_ptr(bytes) as usize))?;
        (self.resolve)(id)
    }
}

/// Deduped feature tags the document used but the translator can't
/// express yet (ordered by first sighting).
#[derive(Default)]
pub(crate) struct Missing(Vec<&'static str>);

impl Missing {
    pub(crate) fn add(&mut self, tag: &'static str) {
        if !self.0.contains(&tag) {
            self.0.push(tag);
        }
    }

    pub(crate) fn into_tags(self) -> Vec<&'static str> {
        self.0
    }
}

pub(crate) fn root(
    tree: &usvg::Tree,
    missing: &mut Missing,
    ids: &HashMap<usize, u32>,
    resolve: &dyn Fn(u32) -> Option<valo_dl::Image>,
) -> DisplayList {
    let mut cx = Ctx {
        missing,
        ids,
        resolve,
    };
    let mut b = DisplayListBuilder::new();
    group(tree.root(), &mut b, &mut cx);
    b.build()
}

fn group(g: &usvg::Group, b: &mut DisplayListBuilder, cx: &mut Ctx) {
    let filter = plan_filter(g, cx);
    // An ink-less mask is coverage 0 everywhere: the group simply isn't.
    if g.mask().is_some_and(mask_hides_everything) {
        return;
    }
    b.save();
    b.concat(&convert::transform(g.transform()));
    if let Some(region) = filter_region(g, &filter) {
        // SVG crops a filtered element's RESULT to the filter region.
        b.clip_rect(region, ClipOp::Intersect);
    }
    let mut clip_masks = Vec::new();
    if let Some(clip) = g.clip_path() {
        if !plan_clips(clip, b, &mut clip_masks) {
            // Clip to nothing (SVG's empty-clip semantics): group hidden.
            b.restore();
            return;
        }
    }
    if let FilterPlan::Shadow {
        dx,
        dy,
        sigma,
        color,
    } = filter
    {
        emit_drop_shadow(g, b, cx, dx, dy, sigma, color);
    }
    let layered = needs_layer(g) || !clip_masks.is_empty() || matches!(filter, FilterPlan::Blur(_));
    if layered {
        let mut paint = layer_paint(g);
        if let FilterPlan::Blur(sigma) = filter {
            paint.mask_blur = Some(valo_dl::MaskBlur {
                sigma,
                style: valo_dl::BlurStyle::Normal,
            });
        }
        b.save_layer(None, &paint);
    }
    for child in g.children() {
        node(child, b, cx);
    }
    for paths in &clip_masks {
        emit_clip_mask(paths, b);
    }
    if let Some(mask) = g.mask() {
        push_mask(mask, b, cx);
    }
    if layered {
        b.restore();
    }
    b.restore();
}

// ── filters (tier 1) ─────────────────────────────────────────────────────

/// The reducible-filter set: a lone isotropic feGaussianBlur becomes the
/// content layer's blur; a lone feDropShadow becomes a blurred, tinted,
/// offset copy under the content. Everything else renders UNFILTERED
/// (flutter_svg's degrade) and tags `filter`. DIVERGENCE (ledgered): SVG
/// filters default to linearRGB interpolation; valo computes in sRGB.
enum FilterPlan {
    None,
    Blur(f32),
    Shadow {
        dx: f32,
        dy: f32,
        sigma: f32,
        color: Color,
    },
}

fn plan_filter(g: &usvg::Group, cx: &mut Ctx) -> FilterPlan {
    let filters = g.filters();
    if filters.is_empty() {
        return FilterPlan::None;
    }
    if let [filter] = filters {
        if let [primitive] = filter.primitives() {
            match primitive.kind() {
                usvg::filter::Kind::GaussianBlur(blur)
                    if *blur.input() == usvg::filter::Input::SourceGraphic =>
                {
                    if let Some(sigma) = isotropic(blur.std_dev_x().get(), blur.std_dev_y().get()) {
                        return FilterPlan::Blur(sigma);
                    }
                }
                usvg::filter::Kind::DropShadow(shadow)
                    if *shadow.input() == usvg::filter::Input::SourceGraphic =>
                {
                    if let Some(sigma) =
                        isotropic(shadow.std_dev_x().get(), shadow.std_dev_y().get())
                    {
                        return FilterPlan::Shadow {
                            dx: shadow.dx(),
                            dy: shadow.dy(),
                            sigma,
                            color: convert::shadow_color(shadow.color(), shadow.opacity().get()),
                        };
                    }
                }
                _ => {}
            }
        }
    }
    cx.add("filter");
    FilterPlan::None
}

/// valo's gaussian is isotropic; matching std-devs reduce, others don't.
fn isotropic(sx: f32, sy: f32) -> Option<f32> {
    ((sx - sy).abs() < 1e-3 && sx > 0.0).then_some(sx)
}

fn filter_region(g: &usvg::Group, plan: &FilterPlan) -> Option<Rect> {
    if matches!(plan, FilterPlan::None) {
        return None;
    }
    let r = g.filters().first()?.rect();
    Some(Rect::new(r.x(), r.y(), r.width(), r.height()))
}

/// The shadow pass: the subtree rendered again into a blurred layer whose
/// pixels are re-tinted to the shadow color by a full-region SrcIn flood
/// (alpha survives, color replaced — then the layer blur softens it).
fn emit_drop_shadow(
    g: &usvg::Group,
    b: &mut DisplayListBuilder,
    cx: &mut Ctx,
    dx: f32,
    dy: f32,
    sigma: f32,
    color: Color,
) {
    let Some(region) = filter_region(g, &FilterPlan::Blur(sigma)) else {
        return;
    };
    b.save();
    b.translate(dx, dy);
    b.save_layer(
        None,
        &Paint {
            mask_blur: Some(valo_dl::MaskBlur {
                sigma,
                style: valo_dl::BlurStyle::Normal,
            }),
            ..Paint::default()
        },
    );
    for child in g.children() {
        node(child, b, cx);
    }
    b.draw_rect(
        region,
        &Paint {
            color,
            blend_mode: valo_dl::BlendMode::SrcIn,
            ..Paint::default()
        },
    );
    b.restore();
    b.restore();
}

/// Group opacity and blend apply to the COMPOSITED subtree, not per draw —
/// they need an offscreen layer. Masks (and mask-desugared union clips)
/// need one too: their DstIn coverage must multiply THIS subtree alone,
/// never the scene behind it. Plain groups skip it (clips and transforms
/// are free on the main pass).
fn needs_layer(g: &usvg::Group) -> bool {
    // NOT usvg's `should_isolate()`: that is resvg's sub-pixmap predicate
    // and includes plain clips, which valo's depth clips handle on the
    // main pass. `isolate()` is the CSS `isolation: isolate` attribute —
    // the only case that's a layer in its own right.
    g.opacity().get() < 1.0
        || g.blend_mode() != usvg::BlendMode::Normal
        || g.isolate()
        || g.mask().is_some()
}

fn layer_paint(g: &usvg::Group) -> Paint {
    Paint {
        color: Color::rgba(1.0, 1.0, 1.0, g.opacity().get()),
        blend_mode: convert::blend_mode(g.blend_mode()),
        ..Paint::default()
    }
}

fn node(n: &usvg::Node, b: &mut DisplayListBuilder, cx: &mut Ctx) {
    match n {
        usvg::Node::Group(g) => group(g, b, cx),
        usvg::Node::Path(p) => path(p, b, cx),
        usvg::Node::Image(img) => image(img, b, cx),
        // Unreachable without usvg's text feature (the parser drops text;
        // the byte pre-scan in `parse` reports it) — belt for when the
        // feature lands.
        usvg::Node::Text(_) => cx.add("text"),
    }
}

/// Embedded images: rasters draw with the host-resolved
/// texture (absent + tagged until it arrives); nested SVG trees translate
/// recursively, scaled into the image box.
fn image(img: &usvg::Image, b: &mut DisplayListBuilder, cx: &mut Ctx) {
    let size = img.size();
    match img.kind() {
        usvg::ImageKind::SVG(tree) => {
            b.save();
            b.scale(
                size.width() / tree.size().width().max(1e-6),
                size.height() / tree.size().height().max(1e-6),
            );
            group(tree.root(), b, cx);
            b.restore();
        }
        usvg::ImageKind::PNG(bytes)
        | usvg::ImageKind::JPEG(bytes)
        | usvg::ImageKind::GIF(bytes)
        | usvg::ImageKind::WEBP(bytes) => match cx.image_for(bytes) {
            Some(texture) => {
                let src = Rect::new(0.0, 0.0, texture.width(), texture.height());
                let dst = Rect::new(0.0, 0.0, size.width(), size.height());
                b.draw_image_rect(
                    &texture,
                    src,
                    dst,
                    Sampling::default(),
                    &Paint::from_color(Color::WHITE),
                );
            }
            None => cx.add("image"),
        },
    }
}

// ── clips ────────────────────────────────────────────────────────────────

/// One clip LEVEL's paths with their baked transforms.
type ClipLevel<'a> = Vec<(usvg::Transform, &'a usvg::Path)>;

/// Depth-clips single-path levels (cheap, on the main pass) and collects
/// multi-path levels for mask desugaring after the children. Nested clips
/// on the clip intersect — each level stands alone. Returns false when a
/// level has no geometry at all: clip to nothing, the group is hidden.
fn plan_clips<'a>(
    clip: &'a usvg::ClipPath,
    b: &mut DisplayListBuilder,
    pending: &mut Vec<ClipLevel<'a>>,
) -> bool {
    if let Some(outer) = clip.clip_path() {
        if !plan_clips(outer, b, pending) {
            return false;
        }
    }
    let mut paths = Vec::new();
    collect_clip_paths(clip.root(), clip.transform(), &mut paths);
    match paths.as_slice() {
        [] => false,
        [(transform, path)] => {
            let baked = convert::path(path.data(), *transform);
            b.clip_path(&baked, clip_rule(path), ClipOp::Intersect);
            true
        }
        _ => {
            pending.push(paths);
            true
        }
    }
}

/// A multi-path clip is a UNION, which stacked Intersect depth clips can't
/// express — but an ALPHA MASK can: fill every clip path white and let the
/// coverage composite do the union (exact even for overlapping paths and
/// mixed fill rules, where geometric merging is wrong).
fn emit_clip_mask(paths: &ClipLevel, b: &mut DisplayListBuilder) {
    b.save_layer_mask(None, MaskKind::Alpha);
    for (transform, path) in paths {
        b.draw_path(
            &convert::path(path.data(), *transform),
            clip_rule(path),
            &Paint::from_color(Color::WHITE),
        );
    }
    b.restore();
}

/// usvg maps `clip-rule` onto the clip path's fill.
fn clip_rule(path: &usvg::Path) -> FillRule {
    path.fill()
        .map(|f| convert::fill_rule(f.rule()))
        .unwrap_or_default()
}

fn collect_clip_paths<'a>(
    g: &'a usvg::Group,
    t: usvg::Transform,
    out: &mut Vec<(usvg::Transform, &'a usvg::Path)>,
) {
    for child in g.children() {
        match child {
            usvg::Node::Path(p) => out.push((t, p)),
            usvg::Node::Group(inner) => {
                collect_clip_paths(inner, t.pre_concat(inner.transform()), out);
            }
            // Only shapes contribute clip geometry (SVG spec); anything
            // else inside a clipPath has nothing to offer.
            usvg::Node::Image(_) | usvg::Node::Text(_) => {}
        }
    }
}

// ── masks ────────────────────────────────────────────────────────────────

/// The mask subtree renders into its own layer whose composite is
/// COVERAGE × DstIn (valo's `save_layer_mask`); the mask's region rect
/// clips it, and a mask on the mask recurses inside — intersection of
/// coverages, exactly SVG's nesting.
fn push_mask(mask: &usvg::Mask, b: &mut DisplayListBuilder, cx: &mut Ctx) {
    b.save_layer_mask(None, mask_kind(mask.kind()));
    let r = mask.rect();
    b.clip_rect(
        Rect::new(r.x(), r.y(), r.width(), r.height()),
        ClipOp::Intersect,
    );
    group(mask.root(), b, cx);
    if let Some(inner) = mask.mask() {
        push_mask(inner, b, cx);
    }
    b.restore();
}

fn mask_kind(kind: usvg::MaskType) -> MaskKind {
    match kind {
        usvg::MaskType::Luminance => MaskKind::Luminance,
        usvg::MaskType::Alpha => MaskKind::Alpha,
    }
}

/// No ink anywhere in the mask chain means coverage 0 for the masked
/// group. Recording the group and letting replay skip the empty mask
/// layer would show it UNMASKED — the exact inversion of the semantics.
fn mask_hides_everything(mask: &usvg::Mask) -> bool {
    mask.root().children().is_empty() || mask.mask().is_some_and(mask_hides_everything)
}

// ── paths ────────────────────────────────────────────────────────────────

fn path(p: &usvg::Path, b: &mut DisplayListBuilder, cx: &mut Ctx) {
    // usvg flattens element transforms into wrapper groups: data is local.
    let geometry = convert::path(p.data(), usvg::Transform::identity());
    match p.paint_order() {
        usvg::PaintOrder::FillAndStroke => {
            fill_path(p, &geometry, b, cx);
            stroke_path(p, &geometry, b, cx);
        }
        usvg::PaintOrder::StrokeAndFill => {
            stroke_path(p, &geometry, b, cx);
            fill_path(p, &geometry, b, cx);
        }
    }
}

fn fill_path(p: &usvg::Path, geometry: &Arc<Path>, b: &mut DisplayListBuilder, cx: &mut Ctx) {
    let Some(fill) = p.fill() else {
        return;
    };
    if let usvg::Paint::Pattern(pat) = fill.paint() {
        let rule = convert::fill_rule(fill.rule());
        let bb = p.bounding_box();
        let bounds = Rect::new(bb.x(), bb.y(), bb.width(), bb.height());
        pattern_fill(b, cx, geometry, rule, fill.opacity().get(), pat, bounds);
        return;
    }
    match convert::fill_paint(fill, cx.missing) {
        Ok((paint, rule)) => b.draw_path(geometry, rule, &paint),
        Err(gap) => cx.add(gap), // this fill skipped; the stroke still draws
    }
}

fn stroke_path(p: &usvg::Path, geometry: &Arc<Path>, b: &mut DisplayListBuilder, cx: &mut Ctx) {
    let Some(stroke) = p.stroke() else {
        return;
    };
    if let usvg::Paint::Pattern(pat) = stroke.paint() {
        let bb = p.stroke_bounding_box();
        let bounds = Rect::new(bb.x(), bb.y(), bb.width(), bb.height());
        pattern_stroke(b, cx, geometry, stroke, pat, bounds);
        return;
    }
    match convert::stroke_paint(stroke, cx.missing) {
        Ok(paint) => b.draw_path(geometry, FillRule::default(), &paint),
        Err(gap) => cx.add(gap),
    }
}

// ── patterns ─────────────────────────────────────────────────────────────

/// Patterns (R1): record the tile subtree ONCE into its own
/// list, then embed it per grid cell over the painted bounds in pattern
/// space — the frame-embed idiom, so tiles share one Arc (tessellated
/// once) and stay crisp at any zoom. Grids past [`MAX_PATTERN_TILES`]
/// belong in a texture tier that doesn't exist yet: the paint is skipped
/// and reported.
const MAX_PATTERN_TILES: i64 = 256;

/// The recorded tile plus its grid over some bounds — shared by fills
/// (clip-to-path) and strokes (alpha-mask coverage).
struct TileGrid {
    tile: Arc<DisplayList>,
    to_pattern: valo_geometry::Matrix,
    rect: (f32, f32, f32, f32),
    ix: std::ops::Range<i64>,
    iy: std::ops::Range<i64>,
}

fn plan_tiles(pat: &usvg::Pattern, bounds: Rect, cx: &mut Ctx) -> Option<TileGrid> {
    if pat.root().children().is_empty() {
        return None; // ink-less pattern paints nothing (not a gap)
    }
    let tile = {
        let mut tb = DisplayListBuilder::new();
        group(pat.root(), &mut tb, cx);
        Arc::new(tb.build())
    };
    let to_pattern = convert::transform(pat.transform());
    let Some(inverse) = to_pattern.invert() else {
        cx.add("pattern-transform");
        return None;
    };
    let grid = inverse.map_rect(&bounds);
    let r = pat.rect();
    let ix0 = ((grid.x - r.x()) / r.width()).floor() as i64;
    let ix1 = ((grid.x + grid.width - r.x()) / r.width()).ceil() as i64;
    let iy0 = ((grid.y - r.y()) / r.height()).floor() as i64;
    let iy1 = ((grid.y + grid.height - r.y()) / r.height()).ceil() as i64;
    if (ix1 - ix0) * (iy1 - iy0) > MAX_PATTERN_TILES {
        cx.add("pattern-tiles");
        return None;
    }
    Some(TileGrid {
        tile,
        to_pattern,
        rect: (r.x(), r.y(), r.width(), r.height()),
        ix: ix0..ix1,
        iy: iy0..iy1,
    })
}

impl TileGrid {
    fn emit(&self, b: &mut DisplayListBuilder) {
        let (x0, y0, w, h) = self.rect;
        b.concat(&self.to_pattern);
        for iy in self.iy.clone() {
            for ix in self.ix.clone() {
                b.save();
                b.translate(x0 + ix as f32 * w, y0 + iy as f32 * h);
                // Tile overflow is hidden (SVG pattern default).
                b.clip_rect(Rect::new(0.0, 0.0, w, h), ClipOp::Intersect);
                b.draw_display_list(&self.tile);
                b.restore();
            }
        }
    }
}

fn pattern_fill(
    b: &mut DisplayListBuilder,
    cx: &mut Ctx,
    geometry: &Arc<Path>,
    rule: FillRule,
    opacity: f32,
    pat: &usvg::Pattern,
    bounds: Rect,
) {
    let Some(grid) = plan_tiles(pat, bounds, cx) else {
        return;
    };
    b.save();
    if opacity < 1.0 {
        b.save_layer(None, &alpha_paint(opacity));
    }
    b.clip_path(geometry, rule, ClipOp::Intersect);
    grid.emit(b);
    if opacity < 1.0 {
        b.restore();
    }
    b.restore();
}

/// A pattern STROKE: no clip-to-stroke geometry exists, but
/// none is needed — tiles paint over the stroke's bounds and the path
/// drawn WHITE in stroke style becomes the coverage of an alpha mask
/// (F3+F4 composing).
fn pattern_stroke(
    b: &mut DisplayListBuilder,
    cx: &mut Ctx,
    geometry: &Arc<Path>,
    stroke: &usvg::Stroke,
    pat: &usvg::Pattern,
    bounds: Rect,
) {
    let Some(grid) = plan_tiles(pat, bounds, cx) else {
        return;
    };
    b.save_layer(None, &alpha_paint(stroke.opacity().get()));
    b.save();
    b.clip_rect(bounds, ClipOp::Intersect);
    grid.emit(b);
    b.restore();
    b.save_layer_mask(None, MaskKind::Alpha);
    b.draw_path(
        geometry,
        FillRule::default(),
        &Paint {
            style: valo_dl::PaintStyle::Stroke(convert::stroke_geometry(stroke)),
            color: Color::WHITE,
            ..Paint::default()
        },
    );
    b.restore();
    b.restore();
}

fn alpha_paint(opacity: f32) -> Paint {
    Paint {
        color: Color::rgba(1.0, 1.0, 1.0, opacity),
        ..Paint::default()
    }
}
