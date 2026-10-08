//! Text: which shape a placed glyph run takes on the GPU. Routing already
//! peeled off the paint's effects, its advanced blend, and the
//! shader-through-a-mask desugar, and handed the run its colour filter,
//! which each glyph's colour passes through before its coverage (Skia's
//! colour filter on the source colour): a coverage glyph's colour folds it
//! on the CPU, a colour glyph's pixels run it in their fragment. What is
//! left is Skia's tier dispatch (`SubRunControl.cpp`) — pick by DEVICE
//! size, then place quads:
//!
//! - huge text fills the glyph's real outline, stencil-then-cover like any
//!   shape, because no atlas entry stays sharp at that size;
//! - pixel-aligned 1:1 text samples plain bitmap masks, the crispest option
//!   and the reason the mask tier snaps to the pixel grid at all;
//! - everything transformed goes through SDFs, where one raster serves a
//!   whole band of scales and rotations.
//!
//! The atlas itself is a cache in `glyphs`; this module only decides what to
//! ask it for and turns the answers into quads.

use std::sync::Arc;

use valo_dl::{GlyphPos, Paint, PaintStyle};
use valo_geometry::{Color, FillRule, Matrix, MatrixKind, Point, Stroke};
use valo_text::{Font, GlyphStroke};

use crate::glyphs::{AtlasGlyph, Coverage, PageRef, TextTiers, SDF_BUCKETS};
use crate::pipelines::{Frag, PipelineBlend};

use super::draw_state::DrawState;
use super::drawing::Drawing;
use super::emit::{Cover, Entity, Role};
use super::primitives::subpixel_stroke_alpha;
use super::shading::{scaled_premul, Shading};
use super::source::{GlyphRun, Shape};

/// Emoji rasters cap here in the outline tier (colour glyphs have no
/// outlines); past it the bitmap upscales, the way Skia clamps glyphs too
/// big for the atlas.
const MAX_COLOR_GLYPH_PX: f32 = 256.0;

/// A stroked mask still has to fit an atlas cell, and the miter reach is
/// unbounded in the paint. Past this the run keeps taking the outline path,
/// where geometry has no size ceiling — the same escape the huge-text tier
/// already is.
const MAX_STROKED_MASK_PX: f32 = 1024.0;

/// `GlyphTier` is which tier a run lands in. The mask tier carries what its
/// entries are keyed on, plus the alpha a floored hairline gives back.
enum GlyphTier {
    Mask { coverage: Coverage, alpha: f32 },
    Sdf,
    Outline,
}

impl GlyphTier {
    /// `stats_index` is where the tier is counted in
    /// [`crate::RenderStats::text_tiers`]: `[bitmap mask, SDF, outline]`.
    fn stats_index(&self) -> usize {
        match self {
            GlyphTier::Mask { .. } => 0,
            GlyphTier::Sdf => 1,
            GlyphTier::Outline => 2,
        }
    }
}

/// `Strike` is one font rastered at one size and coverage: what a glyph's
/// atlas entry is keyed on, besides the glyph and its subpixel phase
/// (Skia's strike).
struct Strike<'f> {
    font: &'f Arc<Font>,
    px: f32,
    coverage: Coverage,
}

impl Drawing<'_, '_> {
    /// `draw_glyphs` picks the run's tier from its DEVICE size and the
    /// paint's style, then hands off to that tier's placement.
    pub fn draw_glyphs(
        &mut self,
        run: &GlyphRun<'_>,
        paint: &Paint,
        at: &DrawState,
        blend: PipelineBlend,
    ) {
        let (font, size, glyphs) = (run.font, run.size, run.glyphs.as_slice());
        // The tiers colour each glyph through the run's colour filter, which
        // rides the paint from here on.
        let paint = &Paint {
            color_filter: run.colour_filter,
            ..paint.clone()
        };
        let device_px = size * at.transform.max_scale();
        let scale = quantize_scale(at.transform.max_scale());
        let tier = glyph_tier(self.caches.glyphs.tiers, paint, scale, device_px);
        self.stats.text_tiers[tier.stats_index()] += 1;
        match tier {
            GlyphTier::Outline => self.draw_glyph_outlines(font, size, paint, glyphs, at, blend),
            GlyphTier::Sdf => {
                let strike = Strike {
                    font,
                    px: sdf_bucket(device_px),
                    coverage: Coverage::Sdf,
                };
                self.draw_glyph_quads(&strike, size, paint, glyphs, at, blend);
            }
            GlyphTier::Mask { coverage, alpha } => {
                let paint = Paint {
                    color: paint.color.with_alpha(paint.color.a * alpha),
                    ..paint.clone()
                };
                let strike = Strike {
                    font,
                    px: size * scale,
                    coverage,
                };
                self.draw_glyph_masks(&strike, size, &paint, glyphs, at, blend);
            }
        }
    }

    /// `draw_glyph_masks` is the mask tier's two shapes: device-snapped
    /// quads when the transform allows it, transformed quads over upright
    /// rasters otherwise (Impeller's shape).
    fn draw_glyph_masks(
        &mut self,
        strike: &Strike,
        size: f32,
        paint: &Paint,
        glyphs: &[GlyphPos],
        at: &DrawState,
        blend: PipelineBlend,
    ) {
        if is_uniform_axis_aligned(&at.transform) {
            self.draw_glyph_quads_snapped(strike, paint, glyphs, at, blend);
        } else {
            self.draw_glyph_quads(strike, size, paint, glyphs, at, blend);
        }
    }

    /// `draw_glyph_quads` places atlas quads in LOCAL space (the SDF tier,
    /// and rotated masks): glyphs of `strike`, placed at `size / px` of
    /// their raster dimensions, with the transform applied by the MVP.
    fn draw_glyph_quads(
        &mut self,
        strike: &Strike,
        size: f32,
        paint: &Paint,
        glyphs: &[GlyphPos],
        at: &DrawState,
        blend: PipelineBlend,
    ) {
        let hide_notdef = self.caches.glyphs.hides_missing_glyphs();
        let shown: Vec<&GlyphPos> = shown_glyphs(glyphs, hide_notdef).collect();
        // Pack first, batch second — packing can GC the pages a batch
        // already points at (see GlyphStore::ensure_run).
        let keys: Vec<(u32, u8)> = shown.iter().map(|g| (g.id, 0)).collect();
        self.ensure_strike(strike, &keys);
        let mut batches: Vec<((Frag, PageRef), Vec<f32>)> = Vec::new();
        for g in shown {
            // A stand-in's per-glyph raster→quad scale makes the mixed-size
            // batch free.
            let Some((raster_px, page, entry)) = self.atlas_glyph(strike, g.id, 0) else {
                continue;
            };
            let batch = batch_for(&mut batches, glyph_frag(page, strike.coverage), page);
            push_glyph_quad(batch, g.x, g.y, &entry, size / raster_px);
        }
        self.push_text_batches(batches, paint, at, blend);
    }

    /// `draw_glyph_quads_snapped` is the crisp path (mask tier,
    /// axis-aligned): glyphs of `strike`, rastered at the quantized device
    /// scale with a quarter-px subpixel phase, quads in DEVICE space 1:1
    /// with their texels, y snapped to the pixel grid — Skia's direct masks,
    /// Impeller's quantized rasters.
    fn draw_glyph_quads_snapped(
        &mut self,
        strike: &Strike,
        paint: &Paint,
        glyphs: &[GlyphPos],
        at: &DrawState,
        blend: PipelineBlend,
    ) {
        let hide_notdef = self.caches.glyphs.hides_missing_glyphs();
        let placed: Vec<(f32, f32, u8, u32)> = shown_glyphs(glyphs, hide_notdef)
            .map(|g| {
                let device = at.transform.map_point(Point::new(g.x, g.y));
                let (x, phase) = snap_quarter(device.x);
                (x, device.y.round(), phase, g.id)
            })
            .collect();
        // Pack first, batch second (see GlyphStore::ensure_run).
        let keys: Vec<(u32, u8)> = placed
            .iter()
            .map(|&(_, _, phase, id)| (id, phase))
            .collect();
        self.ensure_strike(strike, &keys);
        let mut batches: Vec<((Frag, PageRef), Vec<f32>)> = Vec::new();
        for (x, y, phase, id) in placed {
            // Texels land 1:1 when the exact scale is resident; a stand-in
            // from another scale stretches instead (bitmaps re-raster per
            // quantize step, the very churn a hold exists to skip).
            let Some((raster_px, page, entry)) = self.atlas_glyph(strike, id, phase) else {
                continue;
            };
            let batch = batch_for(&mut batches, glyph_frag(page, strike.coverage), page);
            push_glyph_quad(batch, x, y, &entry, strike.px / raster_px);
        }
        self.push_text_batches(batches, paint, &at.with_transform(Matrix::IDENTITY), blend);
    }

    /// `ensure_strike` packs the glyphs `keys` name (glyph id, subpixel
    /// phase) of `strike` into the atlas.
    fn ensure_strike(&mut self, strike: &Strike, keys: &[(u32, u8)]) {
        self.caches
            .glyphs
            .ensure_run(strike.font, strike.px, strike.coverage, keys);
    }

    /// `atlas_glyph` is glyph `id` of `strike` at subpixel `phase` in the
    /// atlas, with the size it was rastered at: the strike's own, or, under
    /// a text-raster hold, the glyph's nearest resident size standing in for
    /// a missing one. `None` when neither is resident.
    fn atlas_glyph(
        &mut self,
        strike: &Strike,
        id: u32,
        phase: u8,
    ) -> Option<(f32, PageRef, AtlasGlyph)> {
        let font = strike.font.uid().0;
        let glyphs = &mut self.caches.glyphs;
        match glyphs.entry(font, id, strike.px, strike.coverage, phase) {
            Some((page, entry)) => Some((strike.px, page, entry)),
            None => glyphs.resident_stand_in(font, id, strike.coverage, strike.px),
        }
    }

    /// `push_text_batches` draws one batch of glyph quads per (fragment,
    /// atlas page), tinted by fragment; `at`'s transform places the quads.
    fn push_text_batches(
        &mut self,
        batches: Vec<((Frag, PageRef), Vec<f32>)>,
        paint: &Paint,
        at: &DrawState,
        blend: PipelineBlend,
    ) {
        for ((frag, page), vertices) in batches {
            let page = self.emit.atlas_bind(&mut self.caches.glyphs, page);
            let entity = Entity {
                stencil: None,
                cover: Cover::Mesh {
                    transform: at.transform,
                    mesh: self.emit.alloc_glyph_mesh(&vertices),
                },
                role: Role::Glyphs,
                shading: glyph_shading(frag, paint, at.alpha, page),
                blend,
                z: at.z,
            };
            self.emit.push(self.context, entity);
        }
    }

    /// `draw_glyph_outlines` is the outline tier: each glyph is a real path,
    /// filled stencil-then-cover like any shape.
    fn draw_glyph_outlines(
        &mut self,
        font: &Arc<Font>,
        size: f32,
        paint: &Paint,
        glyphs: &[GlyphPos],
        at: &DrawState,
        blend: PipelineBlend,
    ) {
        // Shader-painted text desugars into a layer before it gets here, so
        // outlines paint solid, their colour filter folded in — but the STYLE
        // rides along, which is what makes stroked text stroke.
        let outline_paint = Paint {
            color: coverage_colour(paint),
            style: paint.style.clone(),
            ..Default::default()
        };
        let hide_notdef = self.caches.glyphs.hides_missing_glyphs();
        let mut no_outline: Vec<GlyphPos> = Vec::new();
        for g in shown_glyphs(glyphs, hide_notdef) {
            let Some(path) = self.caches.glyphs.path(font, g.id, size) else {
                no_outline.push(*g);
                continue;
            };
            let glyph = at.with_transform(at.transform.then(&Matrix::translation(g.x, g.y)));
            let shape = Shape::of_path(&path, FillRule::NonZero, &outline_paint.style);
            self.draw_shape(&shape, &outline_paint, &glyph, blend);
        }
        // Colour glyphs (emoji) have no outlines — clamp them to the biggest
        // mask raster instead of letting them vanish.
        if !no_outline.is_empty() {
            let px = (size * at.transform.max_scale()).min(MAX_COLOR_GLYPH_PX);
            let bitmap_paint = Paint {
                style: PaintStyle::Fill,
                ..paint.clone()
            };
            let strike = Strike {
                font,
                px,
                coverage: Coverage::Fill,
            };
            self.draw_glyph_quads(&strike, size, &bitmap_paint, &no_outline, at, blend);
        }
    }
}

/// `shown_glyphs` is `glyphs` without glyph 0 (.notdef) when the host hides
/// missing glyphs, to watch `FontDemand` instead of painting tofu; the
/// default draws it, like Skia.
fn shown_glyphs(glyphs: &[GlyphPos], hide_notdef: bool) -> impl Iterator<Item = &GlyphPos> {
    glyphs.iter().filter(move |g| g.id != 0 || !hide_notdef)
}

/// `glyph_tier` is Skia's tier dispatch (`SubRunControl.cpp`), plus the
/// stroke. A stroked run is an ordinary mask-tier run because the rasterizer
/// strokes the outline before rasterizing it — but it never reaches the SDF
/// tier, whose field measures distance from a FILL boundary, and Impeller's
/// stroked glyphs go to the regular atlas for exactly that reason.
fn glyph_tier(tiers: TextTiers, paint: &Paint, scale: f32, device_px: f32) -> GlyphTier {
    if device_px >= tiers.path_min {
        return GlyphTier::Outline;
    }
    let stroke = match &paint.style {
        PaintStyle::Fill if device_px >= tiers.sdf_min => return GlyphTier::Sdf,
        PaintStyle::Fill => {
            return GlyphTier::Mask {
                coverage: Coverage::Fill,
                alpha: 1.0,
            }
        }
        PaintStyle::Stroke(stroke) => stroke,
    };
    match atlas_stroke(stroke, scale, device_px) {
        Some((stroke, alpha)) => GlyphTier::Mask {
            coverage: Coverage::Stroke(stroke),
            alpha,
        },
        None => GlyphTier::Outline,
    }
}

/// `atlas_stroke` is the mask tier's form of a stroke, in the raster's own
/// pixels, with the alpha its floored width owes back. `None` keeps the run
/// on the outline path: a dash is a variable-length pattern that a
/// fixed-size atlas key cannot hold, and a stroke whose miter can reach
/// further than a cell has nowhere to be packed.
fn atlas_stroke(stroke: &Stroke, scale: f32, device_px: f32) -> Option<(GlyphStroke, f32)> {
    if stroke.dash.is_some() {
        return None;
    }
    let device_width = stroke.width * scale;
    let width = device_width.max(1.0);
    // Worst case a join can reach past the glyph's own box, on every side.
    let reach = width * 0.5 * stroke.miter_limit.max(1.0);
    if 2.0 * (device_px + reach) > MAX_STROKED_MASK_PX {
        return None;
    }
    Some((
        GlyphStroke {
            width,
            cap: stroke.cap,
            join: stroke.join,
            miter_limit: stroke.miter_limit,
        },
        subpixel_stroke_alpha(device_width),
    ))
}

/// `sdf_bucket` is Skia's SDF strike bucketing
/// (`SubRunControl::getSDFFont`): raster at the bucket, then reuse it while
/// the device size stays within it.
fn sdf_bucket(device_px: f32) -> f32 {
    for bucket in SDF_BUCKETS {
        if device_px <= bucket {
            return bucket;
        }
    }
    SDF_BUCKETS[SDF_BUCKETS.len() - 1]
}

/// `quantize_scale` is Impeller's mask-tier scale quantization
/// (`text_frame.cc`'s `RoundScaledFontSize`): 1/200 steps, clamped so a
/// glyph always fits the atlas — floating noise dedupes, real zoom
/// re-rasters.
fn quantize_scale(scale: f32) -> f32 {
    ((scale * 200.0).round() / 200.0).clamp(1.0 / 200.0, 48.0)
}

/// `snap_quarter` snaps x to the pixel grid plus a quarter-px phase (Skia's
/// 2-bit subpixel ids, Impeller's `ComputeFractionalPosition`): the raster
/// carries the fraction, the quad sits on the integer.
fn snap_quarter(x: f32) -> (f32, u8) {
    let quarters = (x * 4.0).round();
    let base = (quarters * 0.25).floor();
    let phase = (quarters - base * 4.0) as u8 % 4;
    (base, phase)
}

/// `is_axis_aligned` is scale + translate only, the case where device-space
/// snapping is meaningful. Rotation and flips take the transformed-quad
/// route instead.
fn is_axis_aligned(transform: &Matrix) -> bool {
    transform.kind() == MatrixKind::AxisAligned
}

/// `is_uniform_axis_aligned` additionally requires both axes at the same
/// scale — an anisotropic one cannot place one raster 1:1 on both.
fn is_uniform_axis_aligned(transform: &Matrix) -> bool {
    if !is_axis_aligned(transform) {
        return false;
    }
    let [scale_x, _, _, scale_y, ..] = transform.to_affine();
    (scale_x - scale_y).abs() <= 1e-6 * scale_x.max(scale_y).max(1.0)
}

/// `glyph_frag` is which glyph fragment a page's glyphs need.
fn glyph_frag(page: PageRef, coverage: Coverage) -> Frag {
    match (page.color, coverage) {
        (true, _) => Frag::GlyphColor,
        (false, Coverage::Sdf) => Frag::GlyphSdf,
        (false, _) => Frag::GlyphMask,
    }
}

/// `glyph_shading` is how a batch of `frag` glyphs from `page` is coloured,
/// the colour filter acting on each glyph's colour before its coverage
/// (Skia's `skpaint_to_grpaint_impl`): a coverage glyph is the paint's
/// colour, the filter folded in, times its coverage; a colour glyph keeps
/// its own pixels, which pass through the filter at the paint's alpha
/// (Skia's colour-bitmap text replaces the shader with the glyph).
fn glyph_shading(frag: Frag, paint: &Paint, group_alpha: f32, page: wgpu::BindGroup) -> Shading {
    match frag {
        Frag::GlyphColor => {
            Shading::colour_glyphs(page, paint.color_filter, paint.color.a, group_alpha)
        }
        _ => Shading::glyphs(
            frag,
            scaled_premul(coverage_colour(paint), group_alpha),
            page,
        ),
    }
}

/// `coverage_colour` is the colour a coverage glyph is painted: the paint's,
/// through its colour filter.
fn coverage_colour(paint: &Paint) -> Color {
    paint
        .color_filter
        .map_or(paint.color, |filter| filter.folded_into(paint.color))
}

/// `batch_for` groups quads per (fragment, atlas page) in first-seen order
/// — deterministic emission, one draw per page.
fn batch_for(
    batches: &mut Vec<((Frag, PageRef), Vec<f32>)>,
    frag: Frag,
    page: PageRef,
) -> &mut Vec<f32> {
    let key = (frag, page);
    if let Some(at) = batches.iter().position(|(k, _)| *k == key) {
        return &mut batches[at].1;
    }
    batches.push((key, Vec::new()));
    &mut batches.last_mut().expect("just pushed").1
}

/// `push_glyph_quad` appends one glyph quad at origin (`gx`, `gy`):
/// placement hangs off it (left/top, y-up), uv comes from the atlas slot.
/// `scale` maps raster px → quad units, and is 1.0 in the device-snapped
/// tier, where texels land 1:1.
fn push_glyph_quad(out: &mut Vec<f32>, gx: f32, gy: f32, entry: &AtlasGlyph, scale: f32) {
    let x0 = gx + entry.left * scale;
    let y0 = gy - entry.top * scale;
    let x1 = x0 + entry.width * scale;
    let y1 = y0 + entry.height * scale;
    let [u0, v0, u1, v1] = entry.uv;
    let quad = [
        [x0, y0, u0, v0],
        [x1, y0, u1, v0],
        [x0, y1, u0, v1],
        [x1, y0, u1, v0],
        [x1, y1, u1, v1],
        [x0, y1, u0, v1],
    ];
    for vertex in quad {
        out.extend_from_slice(&vertex);
    }
}

#[cfg(test)]
mod tests {
    use super::is_uniform_axis_aligned;
    use valo_geometry::Matrix;

    /// Only a uniform scale can place one raster 1:1 in device space; an
    /// anisotropic or rotated transform has to take the quad route.
    #[test]
    fn snapped_text_requires_uniform_scale() {
        assert!(is_uniform_axis_aligned(&Matrix::scale(0.5, 0.5)));
        assert!(!is_uniform_axis_aligned(&Matrix::scale(0.5, 1.0)));
        assert!(!is_uniform_axis_aligned(&Matrix::rotation(0.1)));
    }
}
