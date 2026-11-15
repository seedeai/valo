//! Boolean operations on paths: Skia's `Op`, which dart:ui's `Path.combine`
//! calls under both Skia and Impeller, through `valo-pathops`' port of it.
//!
//! A [`Path`] goes to the port as Skia's builder would store the same
//! commands, and the port's answer comes back verb for verb, so a combined
//! path is the one Flutter gets for the same input.

use std::sync::Arc;

use valo_pathops::{SkPath, SkPathBuilder, SkPathFillType, SkPathOp, SkPathVerb, SkPoint};

use crate::{FillRule, Path, PathBuilder, PathElement, Point};

/// `PathOperationError` is the failure of [`Path::combine`]: Skia's `Op`
/// returned no path for these inputs.
///
/// Skia gives up when it cannot resolve the curves' intersections or windings
/// consistently, which happens for some near-coincident or degenerate
/// geometry; no partial result is given. dart:ui's `Path.combine` throws in
/// that case.
pub use valo_pathops::PathOpsError as PathOperationError;

/// `PathOperation` is how [`Path::combine`] joins the areas of two paths:
/// Skia's `SkPathOp`, dart:ui's `PathOperation`.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum PathOperation {
    /// `Difference` is the first path's area outside the second.
    Difference,
    /// `Intersect` is the area inside both paths.
    Intersect,
    /// `Union` is the area inside either path.
    Union,
    /// `Xor` is the area inside exactly one of the paths.
    Xor,
    /// `ReverseDifference` is the second path's area outside the first.
    ReverseDifference,
}

impl Path {
    /// `combine` is the area that `operation` makes of this path, filled with
    /// `fill_rule`, and `other`, filled with `other_fill_rule`, as an outline
    /// to fill with [`FillRule::EvenOdd`]: Skia's `Op`, which dart:ui's
    /// `Path.combine` runs.
    ///
    /// Skia finds where the two paths' curves cross, in doubles and without
    /// flattening them, and writes the pieces that bound the result as closed
    /// contours of lines, quadratics, conics and cubics. The result is the one
    /// Skia gives, point for point; it is a general path
    /// ([`Self::as_oval`] and [`Self::as_round_rect`] are `None`), and empty
    /// when the area is. Use it where an area must be cut from another, such
    /// as a text field's outline around its label or a bottom bar's notch
    /// around a button; for drawing alone, a clip or a second fill is cheaper.
    ///
    /// It fails where Skia's `Op` fails: see [`PathOperationError`].
    pub fn combine(
        &self,
        fill_rule: FillRule,
        operation: PathOperation,
        other: &Path,
        other_fill_rule: FillRule,
    ) -> Result<Arc<Path>, PathOperationError> {
        let result = valo_pathops::op(
            &to_skia_path(self, fill_rule),
            &to_skia_path(other, other_fill_rule),
            to_skia_op(operation),
        )?;
        Ok(from_skia_path(&result))
    }
}

fn to_skia_op(operation: PathOperation) -> SkPathOp {
    match operation {
        PathOperation::Difference => SkPathOp::Difference,
        PathOperation::Intersect => SkPathOp::Intersect,
        PathOperation::Union => SkPathOp::Union,
        PathOperation::Xor => SkPathOp::Xor,
        PathOperation::ReverseDifference => SkPathOp::ReverseDifference,
    }
}

/// `to_skia_path` replays `path` into Skia's builder, which stores it as
/// Skia would have stored the same commands: the two differ only where a
/// valo path holds a move right after a move, which Skia's builder merges.
fn to_skia_path(path: &Path, fill_rule: FillRule) -> SkPath {
    let mut builder = SkPathBuilder::with_fill_type(match fill_rule {
        FillRule::NonZero => SkPathFillType::Winding,
        FillRule::EvenOdd => SkPathFillType::EvenOdd,
    });
    for element in path.elements() {
        match element {
            PathElement::MoveTo(p) => builder.move_to(to_skia_point(p)),
            PathElement::LineTo(p) => builder.line_to(to_skia_point(p)),
            PathElement::QuadTo(c, p) => builder.quad_to(to_skia_point(c), to_skia_point(p)),
            PathElement::ConicTo(c, p, weight) => {
                builder.conic_to(to_skia_point(c), to_skia_point(p), weight)
            }
            PathElement::CubicTo(c1, c2, p) => {
                builder.cubic_to(to_skia_point(c1), to_skia_point(c2), to_skia_point(p))
            }
            PathElement::Close => builder.close(),
        };
    }
    builder.detach()
}

/// `from_skia_path` is Skia's result as a valo path, verb for verb. Every
/// contour Skia writes starts with a move and every conic weight it writes is
/// positive and not 1, so the builder stores the verbs as given.
fn from_skia_path(path: &SkPath) -> Arc<Path> {
    let mut builder = PathBuilder::new();
    let mut points = path.points().iter().map(|p| Point::new(p.x, p.y));
    let mut weights = path.conic_weights().iter();
    let mut next_point = || {
        points
            .next()
            .expect("Skia's path has a point per verb point")
    };
    for verb in path.verbs() {
        match verb {
            SkPathVerb::Move => builder.move_to(next_point()),
            SkPathVerb::Line => builder.line_to(next_point()),
            SkPathVerb::Quad => {
                let control = next_point();
                builder.quad_to(control, next_point())
            }
            SkPathVerb::Conic => {
                let control = next_point();
                let weight = *weights.next().expect("Skia's path has a weight per conic");
                builder.conic_to(control, next_point(), weight)
            }
            SkPathVerb::Cubic => {
                let control1 = next_point();
                let control2 = next_point();
                builder.cubic_to(control1, control2, next_point())
            }
            SkPathVerb::Close => builder.close(),
        };
    }
    builder.build()
}

fn to_skia_point(point: Point) -> SkPoint {
    SkPoint {
        x: point.x,
        y: point.y,
    }
}
