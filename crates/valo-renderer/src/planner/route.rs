//! The one per-draw decision: given a draw request (geometry + paint),
//! pick which execution pattern realizes it — a direct step, an effect
//! layer, or a destination read. Every primitive shares the identical
//! decision, only "draw yourself plain" differs, so the decision lives
//! here once and primitives only know their geometry (Skia's
//! `AutoLayerForImageFilter`, Impeller's
//! `AddRenderEntityWithFiltersToCurrentPass`).
//!
//! Three sources carry deliberate policy differences: an image applies a
//! colour filter on its sampled pixel in the same draw (never a layer), the
//! analytic rrect blur handles its own mask blur in the fragment, and a
//! shader-painted glyph run paints THROUGH its glyphs instead of sampling
//! the shader in their fragments.

use std::sync::Arc;

use valo_dl::{BlendMode, GlyphPos, Image, Paint, PaintStyle, Sampling, Shader};
use valo_geometry::{Color, FillRule, Matrix, Path, Rect};
use valo_text::Font;

use crate::pipelines::{Frag, PipelineKind};

use super::Planner;

/// `DrawSource` is a draw's geometry, decoupled from the decision above it.
pub(super) enum DrawSource<'a> {
    Rect(&'a Rect),
    Path {
        path: &'a Arc<Path>,
        rule: FillRule,
    },
    Image {
        image: &'a Image,
        src: &'a Rect,
        dst: &'a Rect,
        sampling: Sampling,
    },
    /// The recorded fast path for a solid blurred (r)rect — coverage is
    /// analytic in the fragment, so its mask blur never opens a layer.
    RRectBlur {
        rect: &'a Rect,
        radii: [f32; 4],
    },
    Glyphs(GlyphRun<'a>),
}

/// `GlyphRun` is one placed run of glyphs: the font instance, the size it
/// was laid out at, the positioned glyphs, and its recorded ink bounds
/// already mapped into frame coords. Those device bounds ride along because
/// glyph extents are not derivable at plan time — a layer that has to
/// enclose the run sizes itself from them.
#[derive(Clone, Copy)]
pub(super) struct GlyphRun<'a> {
    pub font: &'a Arc<Font>,
    pub size: f32,
    pub glyphs: &'a Arc<Vec<GlyphPos>>,
    pub device_bounds: Rect,
}

impl Planner<'_> {
    /// `plan_routed` inspects one draw request's paint and picks its
    /// execution pattern — the same three-way decision whatever the
    /// geometry:
    ///
    /// - the paint carries a blur or filter that must see this draw's
    ///   FINISHED pixels → render plain into an effect layer, run the
    ///   filter, composite (M3b);
    /// - the blend mode is beyond the fixed-function blend unit → make the
    ///   destination readable and blend in the shader;
    /// - neither → one direct step.
    ///
    /// A colour filter is cheaper than a layer whenever it can fold: into a
    /// solid's colour or a gradient's stops on the CPU, or into an image
    /// draw's sampling fragment.
    ///
    /// `device_bounds` are the RECORDED effect-padded, clip-cropped bounds
    /// mapped into frame coords — effect-layer sizing consumes them instead
    /// of re-deriving `paint.effect_bounds` (rule: drawing facts are
    /// computed at record time).
    pub(super) fn plan_routed(
        &mut self,
        source: DrawSource<'_>,
        paint: &Paint,
        current: &Matrix,
        device_bounds: Rect,
        z: f32,
    ) {
        self.stats.draws += 1;
        if let DrawSource::RRectBlur { rect, radii } = source {
            return self.plan_rrect_blur(rect, radii, paint, current, z);
        }
        // Neither of these folds a colour filter. An image's applies to the
        // SAMPLED pixel in the fragment, not to the (alpha-only) paint
        // colour. A glyph run's applies to the FINISHED run: text is
        // coverage times colour, and filtering the colour first is a
        // different picture wherever the filter is not linear in alpha.
        let folded = match source {
            DrawSource::Image { .. } | DrawSource::Glyphs(_) => None,
            _ => self.prepare_paint(paint),
        };
        let paint = folded.as_ref().unwrap_or(paint);
        if needs_effect_layer(&source, paint) {
            return self.plan_effect_layer(source, paint, current, device_bounds, z);
        }
        if let Some(mode) = advanced_mode(paint) {
            return self.plan_advanced_blend(source, paint, current, z, mode);
        }
        if let Some(run) = masked_glyph_run(&source, paint) {
            return self.plan_masked_glyphs(&run, paint, current, z);
        }
        self.emit_direct(source, paint, current, z);
    }

    /// `plan_masked_glyphs` is the shader-painted-text desugar: the run
    /// draws as a white mask into an implicit layer, the shader fills that
    /// layer `SrcIn` over the run's bounds, and the composite applies the
    /// paint's blend. It is the save-layer recipe a host would write by
    /// hand, and every tier works inside it unchanged.
    fn plan_masked_glyphs(&mut self, run: &GlyphRun<'_>, paint: &Paint, current: &Matrix, z: f32) {
        // The fill quad in LOCAL space. SrcIn masks it down to the glyphs,
        // so a rotated superset of the run's bounds is harmless.
        let local_quad = current.invert().map_or(run.device_bounds, |inverse| {
            inverse.map_rect(&run.device_bounds)
        });
        let fill = Paint {
            shader: paint.shader.clone(),
            color: paint.color,
            blend_mode: BlendMode::SrcIn,
            ..Default::default()
        };
        let (font, glyphs, current2) = (run.font.clone(), run.glyphs.clone(), *current);
        let (size, style) = (run.size, paint.style.clone());
        self.plan_via_implicit_layer(run.device_bounds, z, paint.blend_mode, move |p| {
            // The mask must be drawn the way the paint asks — a stroked
            // gradient headline is stroked coverage, not filled coverage.
            let mask = Paint {
                style: style.clone(),
                ..Paint::from_color(Color::WHITE)
            };
            p.plan_glyph_tiers(&font, size, &mask, &glyphs, &current2, 0.5);
            p.emit_rect_quad(&local_quad, &fill, &current2, 0.5);
        });
    }

    /// `plan_effect_layer` renders the draw plain into its own layer over
    /// its recorded bounds, then the composite runs the paint's colour
    /// filter and blur over that texture. The recorded bounds already carry
    /// the effect padding and the record-time clip crop, which also matches
    /// Impeller's clipped subpass coverage.
    fn plan_effect_layer(
        &mut self,
        source: DrawSource<'_>,
        paint: &Paint,
        current: &Matrix,
        device_bounds: Rect,
        z: f32,
    ) {
        match source {
            DrawSource::Rect(rect) => {
                let (rect, inner_paint, current2) = (*rect, plain(paint), *current);
                self.plan_via_effect_layer_at(device_bounds, paint, current, z, move |p| {
                    p.emit_rect_quad(&rect, &inner_paint, &current2, 0.5);
                });
            }
            DrawSource::Path { path, rule } => {
                let (path, inner_paint, current2) = (path.clone(), plain(paint), *current);
                self.plan_via_effect_layer_at(device_bounds, paint, current, z, move |p| {
                    p.emit_path(&path, rule, &inner_paint, &current2, 0.5);
                });
            }
            DrawSource::Image {
                image,
                src,
                dst,
                sampling,
            } => {
                let (image, src, dst, inner_paint, current2) =
                    (image.clone(), *src, *dst, plain(paint), *current);
                self.plan_via_effect_layer_at(device_bounds, paint, current, z, move |p| {
                    p.emit_image(&image, &src, &dst, sampling, &inner_paint, &current2, 0.5);
                });
            }
            DrawSource::Glyphs(run) => {
                let (font, glyphs, current2) = (run.font.clone(), run.glyphs.clone(), *current);
                let (size, inner_paint) = (run.size, plain(paint));
                self.plan_via_effect_layer_at(device_bounds, paint, current, z, move |p| {
                    p.plan_glyph_tiers(&font, size, &inner_paint, &glyphs, &current2, 0.5);
                });
            }
            DrawSource::RRectBlur { .. } => unreachable!("handled before routing"),
        }
    }

    /// `prepare_paint` folds what the CPU can before any routing: a colour
    /// filter into a solid's colour or a gradient's stops. A pattern's
    /// filter bakes into a cached filtered texture instead (Impeller's
    /// `TiledTextureContents`): filter one immutable source snapshot, then
    /// apply the pattern transform and tile sampler. Mask blur keeps the
    /// filter on the paint — the effect layer needs its ordering.
    fn prepare_paint(&mut self, paint: &Paint) -> Option<Paint> {
        if let Some(folded) = folded_paint(paint) {
            return Some(folded);
        }
        if paint.mask_blur.is_some() {
            return None;
        }
        let filter = paint.color_filter?;
        let Some(Shader::Image { image, .. }) = paint.shader.as_ref() else {
            return None;
        };
        let filtered_image = self.filtered_image(image, filter);
        let mut prepared = paint.clone();
        let Some(Shader::Image { image, .. }) = prepared.shader.as_mut() else {
            unreachable!("source kind changed while cloning paint");
        };
        *image = filtered_image;
        prepared.color_filter = None;
        Some(prepared)
    }

    /// `emit_direct` dispatches the plain draw to its geometry's emitter.
    fn emit_direct(&mut self, source: DrawSource<'_>, paint: &Paint, current: &Matrix, z: f32) {
        match source {
            DrawSource::Rect(rect) => self.emit_rect_quad(rect, paint, current, z),
            DrawSource::Path { path, rule } => self.emit_path(path, rule, paint, current, z),
            DrawSource::Image {
                image,
                src,
                dst,
                sampling,
            } => self.emit_image(image, src, dst, sampling, paint, current, z),
            DrawSource::Glyphs(run) => {
                self.plan_glyph_tiers(run.font, run.size, paint, run.glyphs, current, z)
            }
            DrawSource::RRectBlur { .. } => unreachable!("handled before routing"),
        }
    }

    /// `plan_rrect_blur` is the analytic blurred (r)rect: one quad, no
    /// layer, no filter passes. Advanced blends wrap it in the usual
    /// implicit layer.
    fn plan_rrect_blur(
        &mut self,
        rect: &Rect,
        radii: [f32; 4],
        paint: &Paint,
        current: &Matrix,
        z: f32,
    ) {
        if let Some(mode) = advanced_mode(paint) {
            let device_bounds = current.map_rect(&rect.expand(paint.mask_padding()));
            let (rect, paint, current) = (*rect, paint.clone(), *current);
            self.plan_via_implicit_layer(device_bounds, z, mode, move |p| {
                p.emit_rrect_blur(&rect, radii, &paint, &current, 0.5);
            });
            return;
        }
        self.emit_rrect_blur(rect, radii, paint, current, z);
    }

    /// `plan_advanced_blend` lowers a destination-reading blend: a solid
    /// source snapshots the dst and blends in one fragment (fills keep
    /// stencil-then-cover, with the cover doing the blend); a textured
    /// source first materializes itself in an implicit layer, whose
    /// composite runs the mode.
    fn plan_advanced_blend(
        &mut self,
        source: DrawSource<'_>,
        paint: &Paint,
        current: &Matrix,
        z: f32,
        mode: BlendMode,
    ) {
        match source {
            DrawSource::Rect(rect) if paint.shader.is_none() => {
                let snapshot = self.break_pass(&current.map_rect(rect));
                let group_alpha = self.elision_alpha();
                let frame = self.frames.last_mut().expect("frame stack never empty");
                self.emit.blend_solid_quad(
                    frame,
                    group_alpha,
                    PipelineKind::Draw(Frag::BlendSolid),
                    rect,
                    paint,
                    current,
                    z,
                    mode,
                    &snapshot,
                );
            }
            DrawSource::Path { path, rule }
                if paint.shader.is_none() && matches!(paint.style, PaintStyle::Fill) =>
            {
                let bounds = path.bounds();
                let Some(mesh) = self.stencil_fan_mesh(path, current) else {
                    return;
                };
                // The fan goes in BEFORE the break: the stencil write lands
                // in the earlier segment and survives the pass split (depth/
                // stencil attachments Load across segments); the cover then
                // tests it from the resumed segment.
                let frame = self.frames.last_mut().expect("frame stack never empty");
                self.emit.push_fan(frame, rule, current, mesh, z);
                let snapshot = self.break_pass(&current.map_rect(&bounds));
                let group_alpha = self.elision_alpha();
                let frame = self.frames.last_mut().expect("frame stack never empty");
                self.emit.blend_solid_quad(
                    frame,
                    group_alpha,
                    PipelineKind::Cover(Frag::BlendSolid),
                    &bounds,
                    paint,
                    current,
                    z,
                    mode,
                    &snapshot,
                );
            }
            DrawSource::Rect(rect) => {
                let device_bounds = current.map_rect(rect);
                let (rect, paint, current) = (*rect, paint.clone(), *current);
                self.plan_via_implicit_layer(device_bounds, z, mode, move |p| {
                    p.emit_rect_quad(&rect, &paint, &current, 0.5);
                });
            }
            DrawSource::Path { path, rule } => {
                let padded = path
                    .bounds()
                    .expand(paint.stroke_padding_at_scale(current.max_scale()));
                let device_bounds = current.map_rect(&padded);
                let (path, paint, current) = (path.clone(), plain(paint), *current);
                self.plan_via_implicit_layer(device_bounds, z, mode, move |p| {
                    p.emit_path(&path, rule, &paint, &current, 0.5);
                });
            }
            DrawSource::Image {
                image,
                src,
                dst,
                sampling,
            } => {
                let device_bounds = current.map_rect(dst);
                let (image, src, dst, paint, current) =
                    (image.clone(), *src, *dst, paint.clone(), *current);
                self.plan_via_implicit_layer(device_bounds, z, mode, move |p| {
                    p.emit_image(&image, &src, &dst, sampling, &paint, &current, 0.5);
                });
            }
            DrawSource::Glyphs(run) => {
                let (font, glyphs, current2) = (run.font.clone(), run.glyphs.clone(), *current);
                let (size, inner_paint) = (run.size, plain(paint));
                self.plan_via_implicit_layer(run.device_bounds, z, mode, move |p| {
                    p.plan_glyph_tiers(&font, size, &inner_paint, &glyphs, &current2, 0.5);
                });
            }
            DrawSource::RRectBlur { .. } => unreachable!("handled before routing"),
        }
    }
}

/// `masked_glyph_run` is the run a shader-painted glyph draw has to paint
/// THROUGH. Glyph coverage lives in an atlas that a paint's fragment cannot
/// sample alongside its own colour source, so the run and the shader have to
/// meet in a layer instead of in one draw.
fn masked_glyph_run<'a>(source: &DrawSource<'a>, paint: &Paint) -> Option<GlyphRun<'a>> {
    match source {
        DrawSource::Glyphs(run) if paint.shader.is_some() => Some(*run),
        _ => None,
    }
}

/// `needs_effect_layer` decides whether the paint's remaining effects need
/// the draw's finished pixels. Images are the exception: their colour
/// filter runs inline on the sampled pixel, so only blur-family effects
/// force a layer.
fn needs_effect_layer(source: &DrawSource<'_>, paint: &Paint) -> bool {
    let blur_family = paint.mask_blur.is_some() || paint.effective_image_filter().is_some();
    match source {
        DrawSource::Image { .. } => blur_family,
        _ => blur_family || paint.color_filter.is_some(),
    }
}

/// `advanced_mode` is `Some` when the blend equation cannot run on the
/// fixed-function blend unit, so the shader must read the destination.
fn advanced_mode(paint: &Paint) -> Option<BlendMode> {
    (!paint.blend_mode.is_pipeline_blendable()).then_some(paint.blend_mode)
}

/// `folded_paint` absorbs a colour filter on the CPU, matching Impeller's
/// `Contents::ApplyColorFilter`: a gradient folds it into its stops, a
/// solid into the colour itself. Image patterns return `None` and become
/// cached filtered-source textures instead.
fn folded_paint(paint: &Paint) -> Option<Paint> {
    let filter = paint.color_filter?;
    let mut folded = paint.clone();
    match &mut folded.shader {
        Some(shader) => {
            if !shader.fold_color_filter(&filter) {
                return None;
            }
        }
        None => folded.color = filter.folded_into(paint.color)?,
    }
    folded.color_filter = None;
    Some(folded)
}

/// `plain` is the paint an inner draw of an implicit/effect layer uses:
/// the effects moved to the layer, and the blend deferred to the composite.
fn plain(paint: &Paint) -> Paint {
    Paint {
        blend_mode: BlendMode::SrcOver,
        mask_blur: None,
        color_filter: None,
        image_filter: None,
        ..paint.clone()
    }
}
