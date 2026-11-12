//! Geometry: how each primitive becomes a draw in the top context once the
//! route has already decided direct / layer / dst-read. No routing decisions
//! in here — a primitive only knows how to draw itself plain, with the blend
//! the route gave it.
//!
//! Clips live here as well, because a depth clip is geometry and nothing
//! else: the same stencil-then-cover a fill uses, with the cover writing
//! the scope's expiry depth instead of colour, inside the coverage the clip
//! leaves; a rectangle the scissor states is not even that
//! (`clip_coverage`). It never routes, and it leaves nothing for the
//! matching `Restore` to draw — the recorder already baked the expiry slot
//! into the op, and the coverage a clip returns ends with the scope it is
//! kept in. A mask blur's style clip is the same clip expiring right after
//! the one draw it clips.

use std::sync::Arc;

use valo_dl::{ClipOp, Image, Paint, Sampling};
use valo_geometry::{
    dash_contours, local_tolerance, stroke_strip, FillRule, Matrix, Path, PathBuilder, Rect, Stroke,
};

use crate::frame::Mesh;
use crate::pipelines::{AdvancedBlend, PipelineBlend};

use super::clip_coverage::{ClipCoverage, ClipPlan, ClipShape};
use super::draw_state::DrawState;
use super::drawing::Drawing;
use super::emit::{Cover, Entity, Marking, Role, StencilWrite};
use super::shading::Shading;
use super::source::Shape;

/// `BlendedSolid` is what a solid destination-reading draw covers: a rect,
/// or a filled path's bounds, under the fan its cover tests.
pub(super) enum BlendedSolid {
    Rect(Rect),
    Path {
        bounds: Rect,
        fan: Mesh,
        rule: FillRule,
    },
}

impl Drawing<'_, '_> {
    /// `draw_shape` draws `shape` plain with `paint` and `blend`: a rect as
    /// one quad, a filled path stencil-then-cover, a stroke as a triangle
    /// strip.
    pub fn draw_shape(
        &mut self,
        shape: &Shape<'_>,
        paint: &Paint,
        at: &DrawState,
        blend: PipelineBlend,
    ) {
        match *shape {
            Shape::Rect(rect) => self.draw_rect(&rect, paint, at, blend),
            Shape::Path { path, rule } => self.fill_path(path, rule, paint, at, blend),
            Shape::Stroke { path, stroke } => self.draw_stroke(path, stroke, paint, at, blend),
        }
    }

    /// `draw_rect` is one paint quad covering `rect`.
    fn draw_rect(&mut self, rect: &Rect, paint: &Paint, at: &DrawState, blend: PipelineBlend) {
        let shading = self.emit.paint_shading(paint, at.alpha);
        let entity = Entity {
            stencil: None,
            cover: Cover::Quad {
                transform: at.transform,
                rect: *rect,
            },
            role: Role::Fill,
            shading,
            blend,
            z: at.z,
            clip: at.clip,
        };
        self.emit.push(self.context, entity);
    }

    /// `fill_path` winds the flattened path into the stencil, then one
    /// cover quad draws where it is wound.
    fn fill_path(
        &mut self,
        path: &Arc<Path>,
        rule: FillRule,
        paint: &Paint,
        at: &DrawState,
        blend: PipelineBlend,
    ) {
        let Some(fan) = self.fan_mesh(path, &at.transform) else {
            return;
        };
        let shading = self.emit.paint_shading(paint, at.alpha);
        let entity = Entity {
            stencil: Some(StencilWrite {
                transform: at.transform,
                mesh: fan,
                marking: Marking::Fan(rule),
            }),
            cover: Cover::Quad {
                transform: at.transform,
                rect: path.bounds(),
            },
            role: Role::StencilledFill,
            shading,
            blend,
            z: at.z,
            clip: at.clip,
        };
        self.emit.push(self.context, entity);
    }

    /// `draw_stroke` is one triangle strip along the flattened path
    /// (Impeller's StrokePathGeometry): dash pre-pass, hairline floor,
    /// joins + caps from the stroker. Gradients compose free (local =
    /// position).
    fn draw_stroke(
        &mut self,
        path: &Arc<Path>,
        stroke: &Stroke,
        paint: &Paint,
        at: &DrawState,
        blend: PipelineBlend,
    ) {
        let vertices = self.stroke_vertices(path, stroke, &at.transform);
        if vertices.is_empty() {
            return;
        }
        let coverage = stroke_alpha_coverage(&at.transform, stroke.width);
        let shading = self.emit.paint_shading(paint, at.alpha * coverage);
        let entity = Entity {
            stencil: None,
            cover: Cover::Mesh {
                transform: at.transform,
                mesh: self.emit.alloc_mesh(&vertices),
            },
            role: Role::Stroke,
            shading,
            blend,
            z: at.z,
            clip: at.clip,
        };
        self.emit.push(self.context, entity);
    }

    /// `stroke_vertices` is a stroke's triangle strip in local coordinates.
    /// Impeller renders at least one device pixel of geometry and fades a
    /// positive subpixel stroke to keep its intended coverage
    /// ([`stroke_alpha_coverage`]); a zero-width stroke is a true hairline.
    fn stroke_vertices(&mut self, path: &Arc<Path>, stroke: &Stroke, current: &Matrix) -> Vec<f32> {
        let tolerance = local_tolerance(current);
        let contours = self.caches.contours.contours(path, tolerance);
        let mut stroke = stroke.clone();
        stroke.width = stroke.width.max(1.0 / current.max_scale().max(1e-3));
        match &stroke.dash {
            Some(dash) => {
                let dashed = dash_contours(&contours, dash);
                stroke_strip(&dashed, &stroke, tolerance)
            }
            None => stroke_strip(&contours, &stroke, tolerance),
        }
    }

    /// `draw_image` is one sampled-image draw — the direct path for images,
    /// including an inline colour filter on the sampled pixel. Its alpha is
    /// the paint's alone, the image keeping its own colours.
    #[expect(
        clippy::too_many_arguments,
        reason = "the DrawImage op's fields, the state and the blend"
    )]
    pub fn draw_image(
        &mut self,
        image: &Image,
        src: &Rect,
        dst: &Rect,
        sampling: Sampling,
        paint: &Paint,
        at: &DrawState,
        blend: PipelineBlend,
    ) {
        let alpha = paint.color.a * at.alpha;
        let shading = self
            .emit
            .image_shading(image, src, dst, sampling, paint.color_filter, alpha);
        let entity = Entity {
            stencil: None,
            cover: Cover::Quad {
                transform: at.transform,
                rect: *dst,
            },
            role: Role::Fill,
            shading,
            blend,
            z: at.z,
            clip: at.clip,
        };
        self.emit.push(self.context, entity);
    }

    /// `draw_blended_solid` is the fragment side of a solid advanced blend:
    /// one quad, or a cover over a path's fan, whose fragment runs `mode`
    /// against `destination`, a copy of what the context held beneath it.
    /// The pipeline blend is SrcOver — the result replaces what the copy
    /// captured.
    pub fn draw_blended_solid(
        &mut self,
        solid: BlendedSolid,
        paint: &Paint,
        at: &DrawState,
        mode: AdvancedBlend,
        destination: &wgpu::TextureView,
    ) {
        let size = self.context.area.size();
        let shading =
            self.emit
                .blended_solid_shading(paint.color, at.alpha, mode, destination, size);
        let (stencil, rect, role) = match solid {
            BlendedSolid::Rect(rect) => (None, rect, Role::Fill),
            BlendedSolid::Path { bounds, fan, rule } => {
                let fan = StencilWrite {
                    transform: at.transform,
                    mesh: fan,
                    marking: Marking::Fan(rule),
                };
                (Some(fan), bounds, Role::StencilledFill)
            }
        };
        let entity = Entity {
            stencil,
            cover: Cover::Quad {
                transform: at.transform,
                rect,
            },
            role,
            shading,
            blend: PipelineBlend::SrcOver,
            z: at.z,
            clip: at.clip,
        };
        self.emit.push(self.context, entity);
    }

    /// `draw_rrect_blur` is the analytic blurred (r)rect: one quad over its
    /// blur's spread.
    pub fn draw_rrect_blur(
        &mut self,
        rect: &Rect,
        radii: [f32; 4],
        blur: valo_dl::MaskBlur,
        paint: &Paint,
        at: &DrawState,
        blend: PipelineBlend,
    ) {
        let shading = Shading::rrect_blur(rect, radii, paint.color, blur, at.alpha);
        let entity = Entity {
            stencil: None,
            cover: Cover::Quad {
                transform: at.transform,
                rect: rect.expand((blur.sigma * 3.0).ceil()),
            },
            role: Role::Fill,
            shading,
            blend,
            z: at.z,
            clip: at.clip,
        };
        self.emit.push(self.context, entity);
    }

    /// `clip` clips what is drawn at `at` from here on to the path's
    /// shape, or to outside it, until the depth `at` carries, the clip's
    /// expiry: an Intersect ceiling covers the shape's exterior, a
    /// Difference ceiling its interior. Draws under a ceiling fail the
    /// depth test, and draws recorded after the scope's restore sit above
    /// it — so the clip expires on its own and `Restore` renders nothing.
    /// `shape` is what the clip's plan reads of the path where it lands.
    /// It returns the coverage the clip leaves, which every draw it clips
    /// must be handed.
    pub fn clip(
        &mut self,
        path: &Arc<Path>,
        rule: FillRule,
        op: ClipOp,
        shape: &ClipShape,
        at: &DrawState,
    ) -> ClipCoverage {
        self.clip_by(&path.bounds(), shape, op, at, |drawing| {
            let mesh = drawing.fan_mesh(path, &at.transform)?;
            Some(StencilWrite {
                transform: at.transform,
                mesh,
                marking: Marking::Fan(rule),
            })
        })
    }

    /// `clip_to_shape` clips the next draw, and only it, to `shape` or to
    /// outside it: Impeller's `ApplyClippedBlurStyle`, whose clip takes the
    /// depth of the blur it clips, `at`'s, and is always drawn, whatever its
    /// shape. The ceiling sits half a slot above it, where the next draw is
    /// already past it. It returns the coverage that draw must be handed.
    pub fn clip_to_shape(&mut self, shape: &Shape<'_>, op: ClipOp, at: &DrawState) -> ClipCoverage {
        let expiry = DrawState {
            z: self.context.depth.half_slot_above(at.z),
            ..*at
        };
        match *shape {
            Shape::Rect(rect) => {
                let lands = style_clip_shape(&rect, at);
                self.clip(&rect_path(&rect), FillRule::NonZero, op, &lands, &expiry)
            }
            Shape::Path { path, rule } => {
                let lands = style_clip_shape(&path.bounds(), at);
                self.clip(path, rule, op, &lands, &expiry)
            }
            Shape::Stroke { path, stroke } => self.clip_stroke(path, stroke, op, &expiry),
        }
    }

    /// `clip_stroke` is `clip` for a stroke's geometry: its strip, each
    /// triangle counting into the stencil wherever it lands, as Impeller's
    /// clip counts geometry that may overlap itself (`kStencilIncrementAll`).
    fn clip_stroke(
        &mut self,
        path: &Arc<Path>,
        stroke: &Stroke,
        op: ClipOp,
        at: &DrawState,
    ) -> ClipCoverage {
        let vertices = self.stroke_vertices(path, stroke, &at.transform);
        let bounds = vertex_bounds(&vertices);
        let lands = style_clip_shape(&bounds, at);
        self.clip_by(&bounds, &lands, op, at, |drawing| {
            (!vertices.is_empty()).then(|| StencilWrite {
                transform: at.transform,
                mesh: drawing.emit.alloc_mesh(&vertices),
                marking: Marking::Strip,
            })
        })
    }

    /// `clip_by` clips to a shape of local `bounds`, which lands as `shape`
    /// says, the way its [`ClipPlan`] says: by the scissor alone, or by the
    /// stencil `mark` makes of the shape and a ceiling over it. It returns
    /// the coverage the clip leaves.
    fn clip_by(
        &mut self,
        bounds: &Rect,
        shape: &ClipShape,
        op: ClipOp,
        at: &DrawState,
        mark: impl FnOnce(&mut Self) -> Option<StencilWrite>,
    ) -> ClipCoverage {
        if self.excludes_nothing_visible(bounds, op, &at.transform) {
            self.stats.culled += 1;
            return at.clip;
        }
        match at.clip.plan_clip(op, shape, &self.context.area) {
            ClipPlan::Scissor(coverage) => {
                self.stats.clips += 1;
                coverage
            }
            ClipPlan::Draw(coverage) => {
                let stencil = mark(self);
                self.push_stencil_clip(stencil, bounds, op, &at.clipped_to(coverage));
                coverage
            }
        }
    }

    /// `excludes_nothing_visible` reports whether a clip can be skipped: a
    /// Difference ceiling covers the shape's INTERIOR, so an interior that
    /// lands off-viewport excludes nothing visible. An Intersect clip is
    /// never skipped here: what it leaves is its plan's to say.
    fn excludes_nothing_visible(&self, bounds: &Rect, op: ClipOp, current: &Matrix) -> bool {
        op == ClipOp::Difference && self.context.area.culls(&current.map_rect(bounds))
    }

    /// `push_stencil_clip` marks the clip's shape in the stencil and writes
    /// its ceiling at `at`'s depth, inside `at`'s coverage: over the shape's
    /// exterior for Intersect, or its interior, `bounds`, for Difference.
    /// One draw, which the context keeps to replay after a split.
    fn push_stencil_clip(
        &mut self,
        stencil: Option<StencilWrite>,
        bounds: &Rect,
        op: ClipOp,
        at: &DrawState,
    ) {
        // A zero-AREA shape (a rect collapsed to a line) marks nothing.
        // Intersecting with nothing clips EVERYTHING: with no interior
        // marked, the ceiling covers all the coverage the scope has. An
        // empty Difference excludes nothing — skip.
        if stencil.is_none() && op == ClipOp::Difference {
            return;
        }
        let (cover, role) = match op {
            // Everything outside the shape fails depth until the scope's
            // slots are past.
            ClipOp::Intersect => (Cover::Texels, Role::Clip { difference: false }),
            ClipOp::Difference => (
                Cover::Quad {
                    transform: at.transform,
                    rect: *bounds,
                },
                Role::Clip { difference: true },
            ),
        };
        self.stats.clips += 1;
        let entity = Entity {
            stencil,
            cover,
            role,
            shading: Shading::none(),
            blend: PipelineBlend::SrcOver,
            z: at.z,
            clip: at.clip,
        };
        self.emit.push_clip(self.context, entity);
    }

    /// `fan_mesh` is `path` flattened at the draw's device scale, every
    /// contour fanned from its first point (winding fixes coverage —
    /// triangles may overlap freely); `None` when it has no area to wind.
    pub fn fan_mesh(&mut self, path: &Arc<Path>, current: &Matrix) -> Option<Mesh> {
        let contours = self
            .caches
            .contours
            .contours(path, local_tolerance(current));
        let vertices = fan_vertices(&contours);
        if vertices.is_empty() {
            return None;
        }
        Some(self.emit.alloc_mesh(&vertices))
    }
}

/// `rect_path` is a rectangle as a path, for a clip to it.
fn rect_path(rect: &Rect) -> Arc<Path> {
    let mut builder = PathBuilder::new();
    builder.rect(*rect);
    builder.build()
}

/// `style_clip_shape` is what the plan of a blur style's clip reads of its
/// shape, of local `bounds` and drawn at `at`: where it lands, and that no
/// scissor states it, so it is drawn (Impeller makes its `ClipContents`
/// with `is_axis_aligned_rect` false).
fn style_clip_shape(bounds: &Rect, at: &DrawState) -> ClipShape {
    ClipShape {
        bounds: at.transform.map_rect(bounds),
        is_rect: false,
        anti_alias: true,
    }
}

/// `vertex_bounds` is the rect holding a mesh's `x, y` vertices; empty for
/// none.
fn vertex_bounds(vertices: &[f32]) -> Rect {
    let mut points = vertices.as_chunks::<2>().0.iter();
    let Some(&[mut left, mut top]) = points.next() else {
        return Rect::default();
    };
    let [mut right, mut bottom] = [left, top];
    for &[x, y] in points {
        left = left.min(x);
        top = top.min(y);
        right = right.max(x);
        bottom = bottom.max(y);
    }
    Rect::from_ltrb(left, top, right, bottom)
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
