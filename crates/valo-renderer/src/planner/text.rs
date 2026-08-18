//! Text: which shape a placed glyph run takes on the GPU. Routing already
//! peeled off the paint's effects, its advanced blend, and the
//! shader-through-a-mask desugar, so what is left is Skia's tier dispatch
//! (`SubRunControl.cpp`) — pick by DEVICE size, then place quads:
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
use valo_geometry::{FillRule, Matrix, MatrixKind, Point, Stroke};
use valo_text::{Font, GlyphStroke};

use crate::glyphs::{AtlasGlyph, Coverage, PageRef, TextTiers, SDF_BUCKETS};
use crate::pipelines::TextMode;

use super::emit::{alpha_tint, scaled_premul};
use super::primitives::subpixel_stroke_alpha;
use super::Planner;

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

impl Planner<'_> {
    /// `plan_glyph_tiers` picks the run's tier from its DEVICE size and the
    /// paint's style, then hands off to that tier's placement.
    pub(super) fn plan_glyph_tiers(
        &mut self,
        font: &Arc<Font>,
        size: f32,
        paint: &Paint,
        glyphs: &[GlyphPos],
        current: &Matrix,
        z: f32,
    ) {
        let device_px = size * current.max_scale();
        let scale = quantize_scale(current.max_scale());
        match glyph_tier(self.glyphs.tiers, paint, scale, device_px) {
            GlyphTier::Outline => {
                self.stats.text_tiers[2] += 1;
                self.plan_glyph_outlines(font, size, paint, glyphs, current, z);
            }
            GlyphTier::Sdf => {
                self.stats.text_tiers[1] += 1;
                self.plan_glyph_quads(
                    font,
                    sdf_bucket(device_px),
                    Coverage::Sdf,
                    size,
                    paint,
                    glyphs,
                    current,
                    z,
                );
            }
            GlyphTier::Mask { coverage, alpha } => {
                self.stats.text_tiers[0] += 1;
                let paint = Paint {
                    color: paint.color.with_alpha(paint.color.a * alpha),
                    ..paint.clone()
                };
                self.plan_glyph_masks(font, size, scale, coverage, &paint, glyphs, current, z);
            }
        }
    }

    /// `plan_glyph_masks` is the mask tier's two shapes: device-snapped
    /// quads when the transform allows it, transformed quads over upright
    /// rasters otherwise (Impeller's shape).
    #[expect(clippy::too_many_arguments, reason = "mirrors plan_glyph_tiers")]
    fn plan_glyph_masks(
        &mut self,
        font: &Arc<Font>,
        size: f32,
        scale: f32,
        coverage: Coverage,
        paint: &Paint,
        glyphs: &[GlyphPos],
        current: &Matrix,
        z: f32,
    ) {
        if is_uniform_axis_aligned(current) {
            self.plan_glyph_quads_snapped(font, scale, size, coverage, paint, glyphs, current, z);
        } else {
            self.plan_glyph_quads(
                font,
                size * scale,
                coverage,
                size,
                paint,
                glyphs,
                current,
                z,
            );
        }
    }

    /// `plan_glyph_quads` places atlas quads in LOCAL space (the SDF tier,
    /// and rotated masks): glyphs rastered at `px`, placed at `size/px` of
    /// their raster dimensions, with the transform applied by the MVP.
    #[expect(clippy::too_many_arguments, reason = "mirrors the GlyphRun op + tier")]
    fn plan_glyph_quads(
        &mut self,
        font: &Arc<Font>,
        px: f32,
        coverage: Coverage,
        size: f32,
        paint: &Paint,
        glyphs: &[GlyphPos],
        current: &Matrix,
        z: f32,
    ) {
        // Pack first, batch second — packing can GC the pages a batch
        // already points at (see GlyphStore::ensure_run). Hosts may opt to
        // hide glyph 0 (.notdef) and watch FontDemand instead of painting
        // tofu; the default draws it, like Skia.
        let hide_notdef = self.glyphs.hides_missing_glyphs();
        let keys: Vec<(u32, u8)> = glyphs
            .iter()
            .filter(|g| g.id != 0 || !hide_notdef)
            .map(|g| (g.id, 0))
            .collect();
        self.glyphs.ensure_run(font, px, coverage, &keys);
        let mut batches: Vec<((TextMode, PageRef), Vec<f32>)> = Vec::new();
        for g in glyphs.iter().filter(|g| g.id != 0 || !hide_notdef) {
            // Under a text-raster hold, a missing size draws through the
            // glyph's nearest resident size, scaled — the per-glyph
            // raster→quad scale makes the mixed-size batch free.
            let (got_px, page, entry) = match self.glyphs.entry(font.uid().0, g.id, px, coverage, 0)
            {
                Some((page, entry)) => (px, page, entry),
                None => match self
                    .glyphs
                    .resident_stand_in(font.uid().0, g.id, coverage, px)
                {
                    Some(hit) => hit,
                    None => continue,
                },
            };
            let batch = batch_for(&mut batches, text_mode(page, coverage), page);
            push_glyph_quad(batch, g.x, g.y, &entry, size / got_px);
        }
        self.push_text_batches(batches, paint, current, z);
    }

    /// `plan_glyph_quads_snapped` is the crisp path (mask tier,
    /// axis-aligned): glyphs rastered at the quantized device scale with a
    /// quarter-px subpixel phase, quads in DEVICE space 1:1 with their
    /// texels, y snapped to the pixel grid — Skia's direct masks, Impeller's
    /// quantized rasters.
    #[expect(clippy::too_many_arguments, reason = "mirrors the GlyphRun op + tier")]
    fn plan_glyph_quads_snapped(
        &mut self,
        font: &Arc<Font>,
        scale: f32,
        size: f32,
        coverage: Coverage,
        paint: &Paint,
        glyphs: &[GlyphPos],
        current: &Matrix,
        z: f32,
    ) {
        let px = size * scale;
        // Same optional .notdef policy as plan_glyph_quads.
        let hide_notdef = self.glyphs.hides_missing_glyphs();
        let placed: Vec<(f32, f32, u8, u32)> = glyphs
            .iter()
            .filter(|g| g.id != 0 || !hide_notdef)
            .map(|g| {
                let device = current.map_point(Point::new(g.x, g.y));
                let (x, phase) = snap_quarter(device.x);
                (x, device.y.round(), phase, g.id)
            })
            .collect();
        // Pack first, batch second (see GlyphStore::ensure_run).
        let keys: Vec<(u32, u8)> = placed
            .iter()
            .map(|&(_, _, phase, id)| (id, phase))
            .collect();
        self.glyphs.ensure_run(font, px, coverage, &keys);
        let mut batches: Vec<((TextMode, PageRef), Vec<f32>)> = Vec::new();
        for (x, y, phase, id) in placed {
            // Texels land 1:1 when the exact scale is resident; under a
            // hold, a stand-in from another scale stretches instead
            // (bitmaps re-raster per quantize step, the very churn the hold
            // exists to skip).
            let (scale, page, entry) =
                match self.glyphs.entry(font.uid().0, id, px, coverage, phase) {
                    Some((page, entry)) => (1.0, page, entry),
                    None => match self
                        .glyphs
                        .resident_stand_in(font.uid().0, id, coverage, px)
                    {
                        Some((got_px, page, entry)) => (px / got_px, page, entry),
                        None => continue,
                    },
                };
            let batch = batch_for(&mut batches, text_mode(page, coverage), page);
            push_glyph_quad(batch, x, y, &entry, scale);
        }
        self.push_text_batches(batches, paint, &Matrix::IDENTITY, z);
    }

    /// `push_text_batches` emits one step per (mode, atlas page) batch,
    /// tinted by mode.
    fn push_text_batches(
        &mut self,
        batches: Vec<((TextMode, PageRef), Vec<f32>)>,
        paint: &Paint,
        model: &Matrix,
        z: f32,
    ) {
        for ((mode, page), vertices) in batches {
            let tint = text_tint(mode, paint, self.elision_alpha());
            let mesh = self.emit.alloc_text_mesh(&vertices);
            let bind = self.emit.atlas_bind(self.glyphs, page);
            let frame = self.frames.last_mut().expect("frame stack never empty");
            self.emit
                .text_step(frame, mode, tint, paint.blend_mode, model, mesh, bind, z);
        }
    }

    /// `plan_glyph_outlines` is the outline tier: each glyph is a real path,
    /// filled stencil-then-cover like any shape.
    fn plan_glyph_outlines(
        &mut self,
        font: &Arc<Font>,
        size: f32,
        paint: &Paint,
        glyphs: &[GlyphPos],
        current: &Matrix,
        z: f32,
    ) {
        // Shader-painted text desugars into a layer before it gets here, so
        // outlines paint solid — but the STYLE rides along, which is what
        // makes stroked text stroke.
        let paint = Paint {
            color: paint.color,
            blend_mode: paint.blend_mode,
            style: paint.style.clone(),
            ..Default::default()
        };
        let hide_notdef = self.glyphs.hides_missing_glyphs();
        let mut no_outline: Vec<GlyphPos> = Vec::new();
        for g in glyphs.iter().filter(|g| g.id != 0 || !hide_notdef) {
            let Some(path) = self.glyphs.path(font, g.id, size) else {
                no_outline.push(*g);
                continue;
            };
            let at = current.then(&Matrix::translation(g.x, g.y));
            self.emit_path(&path, FillRule::NonZero, &paint, &at, z);
        }
        // Colour glyphs (emoji) have no outlines — clamp them to the biggest
        // mask raster instead of letting them vanish.
        if !no_outline.is_empty() {
            let px = (size * current.max_scale()).min(MAX_COLOR_GLYPH_PX);
            let bitmap_paint = Paint {
                style: PaintStyle::Fill,
                ..paint.clone()
            };
            self.plan_glyph_quads(
                font,
                px,
                Coverage::Fill,
                size,
                &bitmap_paint,
                &no_outline,
                current,
                z,
            );
        }
    }
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

/// `text_mode` is which text fragment a page's glyphs need.
fn text_mode(page: PageRef, coverage: Coverage) -> TextMode {
    match (page.color, coverage) {
        (true, _) => TextMode::Color,
        (false, Coverage::Sdf) => TextMode::Sdf,
        (false, _) => TextMode::Mask,
    }
}

/// `text_tint` is what multiplies a batch's fragments. Colour glyphs keep
/// their own palette, so only alpha rides their tint.
fn text_tint(mode: TextMode, paint: &Paint, group_alpha: f32) -> [f32; 4] {
    match mode {
        TextMode::Color => alpha_tint(paint.color.a * group_alpha),
        _ => scaled_premul(paint.color, group_alpha),
    }
}

/// `batch_for` groups quads per (mode, atlas page) in first-seen order —
/// deterministic step emission, one draw per page.
fn batch_for(
    batches: &mut Vec<((TextMode, PageRef), Vec<f32>)>,
    mode: TextMode,
    page: PageRef,
) -> &mut Vec<f32> {
    let key = (mode, page);
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
