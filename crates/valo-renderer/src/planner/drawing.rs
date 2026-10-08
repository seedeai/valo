//! The view every draw goes through. [`Drawing`] borrows the top context,
//! the step emitter, the caches a draw reads (glyph atlases, contours) and
//! the frame's stats — and nothing else, so a draw appends to the top
//! context and provably cannot open a context or add a pass. Whatever needs
//! either (a layer, a destination read, a filter pass) is orchestration, and
//! stays on the planner.
//!
//! Its geometry lives in `primitives`, its text in `text`, and the drawing
//! of a filter tree's output in `filter_output`.

use valo_dl::Paint;

use crate::pipelines::PipelineBlend;
use crate::renderer::RenderStats;

use super::draw_context::DrawContext;
use super::draw_state::DrawState;
use super::emit::StepEmitter;
use super::source::{fill_paint, white_mask, DrawSource, GlyphRun, Shape};
use super::Caches;

/// `Drawing` appends draws to one draw context.
pub(super) struct Drawing<'p, 'a> {
    pub context: &'p mut DrawContext,
    pub emit: &'p mut StepEmitter<'a>,
    pub caches: &'p mut Caches,
    pub stats: &'p mut RenderStats,
}

impl Drawing<'_, '_> {
    /// `draw` draws `source` plain with `paint` at `at`, blending by
    /// `blend`: the one dispatch from what a draw draws to its emitter.
    pub fn draw(
        &mut self,
        source: DrawSource<'_>,
        paint: &Paint,
        at: &DrawState,
        blend: PipelineBlend,
    ) {
        match source {
            DrawSource::Shape { shape, .. } => self.draw_shape(&shape, paint, at, blend),
            DrawSource::Image {
                image,
                src,
                dst,
                sampling,
            } => self.draw_image(image, &src, &dst, sampling, paint, at, blend),
            DrawSource::RRectBlur { rect, radii, blur } => {
                self.draw_rrect_blur(&rect, radii, blur, paint, at, blend)
            }
            DrawSource::Glyphs(run) => self.draw_glyphs(&run, paint, at, blend),
        }
    }

    /// `draw_alone` draws `source` with `paint` at `at` into a layer that
    /// holds it alone, as it draws without its effects and its blend:
    /// shader-painted glyphs as their coverage filled with the shader, any
    /// other draw as [`Drawing::draw`] draws it.
    pub fn draw_alone(&mut self, source: DrawSource<'_>, paint: &Paint, at: &DrawState) {
        match source {
            DrawSource::Glyphs(run) if paint.shader.is_some() => {
                self.draw_shaded_glyphs(&run, paint, at)
            }
            source => self.draw(source, paint, at, PipelineBlend::SrcOver),
        }
    }

    /// `draw_shaded_glyphs` is the shader-painted-text desugar, into a layer
    /// that holds nothing else: the run draws as a white mask, and the
    /// shader fills its ink `SrcIn`. It is the save-layer recipe a host
    /// would write by hand, and every tier works inside it unchanged.
    fn draw_shaded_glyphs(&mut self, run: &GlyphRun<'_>, paint: &Paint, at: &DrawState) {
        // The mask must be drawn the way the paint asks — a stroked gradient
        // headline is stroked coverage, not filled coverage.
        self.draw_glyphs(run, &white_mask(paint), at, PipelineBlend::SrcOver);
        let ink = Shape::Rect(run.content_bounds);
        self.draw_shape(&ink, &fill_paint(paint), at, PipelineBlend::SrcIn);
    }
}
