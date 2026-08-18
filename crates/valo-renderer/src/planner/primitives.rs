//! Geometry emitters: how each primitive becomes GPU work once the route
//! has already decided direct / layer / dst-read. No routing decisions in
//! here — a primitive only knows how to draw itself plain.
//!
//! Clips live here as well, because a depth clip is geometry and nothing
//! else: the same stencil-then-cover a fill uses, with the cover writing
//! the scope's expiry depth instead of colour. It never routes, and it
//! leaves nothing for the matching `Restore` to undo — the recorder already
//! baked the expiry slot into the op.

use std::sync::Arc;

use valo_dl::{ClipOp, Image, Paint, Sampling};
use valo_geometry::{
    dash_contours, local_tolerance, stroke_strip, FillRule, Matrix, Path, Rect, Stroke,
};

use crate::host_buffer::VertexSlot;
use crate::pipelines::PipelineKind;

use super::emit::{paint_frag, tinted};
use super::Planner;

impl Planner<'_> {
    /// `emit_rect_quad` is one paint quad covering `rect` — the direct path
    /// for rectangles.
    pub(super) fn emit_rect_quad(&mut self, rect: &Rect, paint: &Paint, current: &Matrix, z: f32) {
        let group_alpha = self.elision_alpha();
        let frame = self.frames.last_mut().expect("frame stack never empty");
        self.emit.paint_quad(
            frame,
            group_alpha,
            PipelineKind::Draw(paint_frag(paint)),
            rect,
            paint,
            current,
            z,
        );
    }

    /// `emit_path` draws a path plain: fills via stencil-then-cover (wind
    /// the flattened path into the stencil, then one cover quad draws where
    /// wound), strokes via a CPU triangle strip.
    pub(super) fn emit_path(
        &mut self,
        path: &Arc<Path>,
        rule: FillRule,
        paint: &Paint,
        current: &Matrix,
        z: f32,
    ) {
        let stroke = match &paint.style {
            valo_dl::PaintStyle::Fill => {
                let Some(mesh) = self.stencil_fan_mesh(path, current) else {
                    return;
                };
                let group_alpha = self.elision_alpha();
                let frame = self.frames.last_mut().expect("frame stack never empty");
                self.emit.push_fan(frame, rule, current, mesh, z);
                self.emit.paint_quad(
                    frame,
                    group_alpha,
                    PipelineKind::Cover(paint_frag(paint)),
                    &path.bounds(),
                    paint,
                    current,
                    z,
                );
                return;
            }
            valo_dl::PaintStyle::Stroke(stroke) => stroke.clone(),
        };
        self.emit_path_stroke(path, &stroke, paint, current, z);
    }

    /// `emit_path_stroke` is one `Strip` step along the flattened path
    /// (Impeller's StrokePathGeometry): dash pre-pass, hairline floor,
    /// joins + caps from the stroker. Gradients compose free (local =
    /// position).
    fn emit_path_stroke(
        &mut self,
        path: &Arc<Path>,
        stroke: &Stroke,
        paint: &Paint,
        current: &Matrix,
        z: f32,
    ) {
        let tolerance = local_tolerance(current);
        let contours = self.contours.contours(path, tolerance);
        // Impeller renders at least one device pixel of geometry, then fades
        // positive subpixel strokes to preserve their intended coverage. A
        // zero-width stroke is a true hairline and remains fully opaque.
        let mut stroke = stroke.clone();
        let coverage = stroke_alpha_coverage(current, stroke.width);
        stroke.width = stroke.width.max(1.0 / current.max_scale().max(1e-3));
        let vertices = match &stroke.dash {
            Some(dash) => {
                let dashed = dash_contours(&contours, dash);
                stroke_strip(&dashed, &stroke, tolerance)
            }
            None => stroke_strip(&contours, &stroke, tolerance),
        };
        if vertices.is_empty() {
            return;
        }
        let tint = tinted(paint, self.elision_alpha() * coverage);
        let mesh = self.emit.alloc_mesh(&vertices);
        let frame = self.frames.last_mut().expect("frame stack never empty");
        self.emit.strip_step(frame, tint, paint, current, mesh, z);
    }

    /// `emit_image` is one sampled-image draw — the direct path for images,
    /// including an inline colour filter on the sampled pixel.
    #[expect(
        clippy::too_many_arguments,
        reason = "mirrors the DrawImage op's fields 1:1"
    )]
    pub(super) fn emit_image(
        &mut self,
        image: &Image,
        src: &Rect,
        dst: &Rect,
        sampling: Sampling,
        paint: &Paint,
        current: &Matrix,
        z: f32,
    ) {
        let group_alpha = self.elision_alpha();
        let frame = self.frames.last_mut().expect("frame stack never empty");
        self.emit.image_step(
            frame,
            group_alpha,
            image,
            src,
            dst,
            sampling,
            paint,
            current,
            z,
        );
    }

    /// `emit_rrect_blur` is the analytic blurred (r)rect quad.
    pub(super) fn emit_rrect_blur(
        &mut self,
        rect: &Rect,
        radii: [f32; 4],
        paint: &Paint,
        current: &Matrix,
        z: f32,
    ) {
        let group_alpha = self.elision_alpha();
        let frame = self.frames.last_mut().expect("frame stack never empty");
        self.emit
            .rrect_blur_step(frame, group_alpha, rect, radii, paint, current, z);
    }

    /// `plan_clip` stencils the shape and writes a depth CEILING at the
    /// clip's expiry z: an Intersect ceiling covers the shape's exterior, a
    /// Difference ceiling its interior. Draws under a ceiling fail the depth
    /// test, and draws recorded after the scope's restore sit above it — so
    /// the clip expires on its own and `Restore` renders nothing.
    pub(super) fn plan_clip(
        &mut self,
        path: &Arc<Path>,
        rule: FillRule,
        op: ClipOp,
        current: &Matrix,
        z: f32,
    ) {
        // A Difference ceiling covers the shape's INTERIOR, so an interior
        // that lands off-viewport excludes nothing visible and the stencil
        // plus ceiling can be skipped outright. An Intersect ceiling covers
        // the exterior and can never be culled.
        if op == ClipOp::Difference {
            let visible = current
                .map_rect(&path.bounds())
                .intersects(&self.frame().cull_rect);
            if !visible {
                self.stats.culled += 1;
                return;
            }
        }
        let Some(mesh) = self.stencil_fan_mesh(path, current) else {
            // A zero-AREA path (a rect collapsed to a line) fans no
            // triangles. Intersecting with nothing clips EVERYTHING: with no
            // interior marked, the full-frame ceiling covers the whole
            // scope. An empty Difference excludes nothing — skip.
            if op == ClipOp::Intersect {
                self.stats.clips += 1;
                self.push_intersect_ceiling(z);
            }
            return;
        };
        self.stats.clips += 1;
        let frame = self.frames.last_mut().expect("frame stack never empty");
        self.emit.push_fan(frame, rule, current, mesh, z);
        match op {
            ClipOp::Intersect => self.push_intersect_ceiling(z),
            ClipOp::Difference => {
                let bounds = path.bounds();
                let frame = self.frames.last_mut().expect("frame stack never empty");
                self.emit.clip_cover_step(frame, &bounds, current, z);
            }
        }
    }

    /// `push_intersect_ceiling` writes the Intersect clip's ceiling over the
    /// whole frame — everything outside the shape fails depth until the
    /// scope's slots are past.
    fn push_intersect_ceiling(&mut self, z: f32) {
        let frame = self.frames.last_mut().expect("frame stack never empty");
        self.emit.clip_ceiling_step(frame, z);
    }

    /// `stencil_fan_mesh` flattens at the draw's device scale and fans every
    /// contour from its first point (winding fixes coverage — triangles may
    /// overlap freely).
    pub(super) fn stencil_fan_mesh(
        &mut self,
        path: &Arc<Path>,
        current: &Matrix,
    ) -> Option<(VertexSlot, u32)> {
        let contours = self.contours.contours(path, local_tolerance(current));
        let vertices = fan_vertices(&contours);
        if vertices.is_empty() {
            return None;
        }
        Some(self.emit.alloc_mesh(&vertices))
    }
}

/// `fan_vertices` builds a triangle-list fan per contour: (p0, pi, pi+1).
/// Overlap and orientation are fine — the stencil winding sorts coverage
/// out.
fn fan_vertices(contours: &[valo_geometry::Contour]) -> Vec<f32> {
    let triangles: usize = contours
        .iter()
        .map(|c| c.points.len().saturating_sub(2))
        .sum();
    let mut out = Vec::with_capacity(triangles * 6);
    for contour in contours {
        let contour = &contour.points;
        let p0 = contour[0];
        for pair in contour[1..].windows(2) {
            out.extend_from_slice(&[p0.x, p0.y, pair[0].x, pair[0].y, pair[1].x, pair[1].y]);
        }
    }
    out
}

/// `stroke_alpha_coverage` is Impeller's `Geometry::ComputeStrokeAlphaCoverage`:
/// geometry below one device pixel is widened, so positive-width strokes
/// compensate in alpha. Width zero deliberately means a fully covered
/// one-pixel hairline.
fn stroke_alpha_coverage(transform: &Matrix, width: f32) -> f32 {
    subpixel_stroke_alpha(transform.max_scale() * width)
}

/// `subpixel_stroke_alpha` is the same compensation for a width already in
/// device pixels — the text mask tier floors its RASTER width rather than
/// its geometry, so it owes the alpha back at that point instead.
pub(super) fn subpixel_stroke_alpha(device_width: f32) -> f32 {
    if device_width == 0.0 || device_width >= 1.0 {
        1.0
    } else {
        (device_width * 2.0).clamp(0.0, 1.0)
    }
}

#[cfg(test)]
mod tests {
    use super::stroke_alpha_coverage;
    use valo_geometry::Matrix;

    #[test]
    fn hairline_coverage_matches_impeller() {
        assert_eq!(stroke_alpha_coverage(&Matrix::IDENTITY, 0.0), 1.0);
        assert_eq!(stroke_alpha_coverage(&Matrix::IDENTITY, 0.25), 0.5);
        assert_eq!(stroke_alpha_coverage(&Matrix::IDENTITY, 0.5), 1.0);
        assert_eq!(stroke_alpha_coverage(&Matrix::scale(2.0, 2.0), 0.25), 1.0);
    }
}
