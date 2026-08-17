use std::sync::Arc;

use valo::{Dash, Path, PathBuilder, Point, Rect, Stroke, Winding};
use wasm_bindgen::prelude::*;

use crate::types;

/// `WebPath` is a mutable collection of line and Bézier contours.
///
/// Use it to describe shapes for drawing, clipping, and hit-testing. Coordinates
/// are local pixels with y downward. Display lists snapshot the path at record
/// time, so later edits do not change already recorded commands.
#[wasm_bindgen(js_name = Path)]
pub struct WebPath {
    builder: PathBuilder,
    cached: Option<Arc<Path>>,
}

impl WebPath {
    pub(crate) fn built(&mut self) -> Arc<Path> {
        self.cached
            .get_or_insert_with(|| self.builder.clone().build())
            .clone()
    }

    fn change(&mut self, operation: impl FnOnce(&mut PathBuilder)) {
        operation(&mut self.builder);
        self.cached = None;
    }
}

#[wasm_bindgen(js_class = Path)]
impl WebPath {
    /// `new` creates an empty path.
    #[wasm_bindgen(constructor)]
    pub fn new() -> Self {
        Self {
            builder: PathBuilder::new(),
            cached: None,
        }
    }

    /// `clone` returns a retained snapshot for Canvas state such as clipping.
    ///
    /// Subsequent edits to either path do not affect the other.
    #[wasm_bindgen(js_name = clone)]
    pub fn duplicate(&self) -> Self {
        Self {
            builder: self.builder.clone(),
            cached: self.cached.clone(),
        }
    }

    /// `moveTo` starts a new contour at `(x, y)`.
    #[wasm_bindgen(js_name = moveTo)]
    pub fn move_to(&mut self, x: f32, y: f32) {
        self.change(|path| {
            path.move_to((x, y));
        });
    }

    /// `lineTo` adds a straight segment to `(x, y)`.
    #[wasm_bindgen(js_name = lineTo)]
    pub fn line_to(&mut self, x: f32, y: f32) {
        self.change(|path| {
            path.line_to((x, y));
        });
    }

    /// `quadraticCurveTo` adds a quadratic Bézier through one control point to `(x, y)`.
    #[wasm_bindgen(js_name = quadraticCurveTo)]
    pub fn quadratic_curve_to(&mut self, control_x: f32, control_y: f32, x: f32, y: f32) {
        self.change(|path| {
            path.quad_to((control_x, control_y), (x, y));
        });
    }

    /// `bezierCurveTo` adds a cubic Bézier through two control points to `(x, y)`.
    #[wasm_bindgen(js_name = bezierCurveTo)]
    pub fn bezier_curve_to(
        &mut self,
        control1_x: f32,
        control1_y: f32,
        control2_x: f32,
        control2_y: f32,
        x: f32,
        y: f32,
    ) {
        self.change(|path| {
            path.cubic_to((control1_x, control1_y), (control2_x, control2_y), (x, y));
        });
    }

    /// `close` adds a segment back to the current contour's starting point.
    ///
    /// It has no effect when no contour is open.
    pub fn close(&mut self) {
        self.change(|path| {
            path.close();
        });
    }

    /// `rect` adds a closed rectangular contour.
    pub fn rect(&mut self, x: f32, y: f32, width: f32, height: f32) {
        self.change(|path| {
            path.rect(Rect::new(x, y, width, height));
        });
    }

    /// `roundRect` adds a closed rounded rectangle with explicit winding.
    ///
    /// `radii` must contain 1, 4, or 8 values: one radius for every corner, four
    /// circular radii clockwise from the top-left, or eight elliptical
    /// `[x, y]` pairs in that same corner order. Any other length throws.
    /// Adjacent radii that would overlap are proportionally reduced.
    ///
    /// `counterclockwise` comes from Canvas2D's sign-parity rule: a
    /// `roundRect` with exactly one negative extent is traversed the other
    /// way, and under the non-zero fill rule that makes it subtract from an
    /// overlapping rectangle rather than add to it. The box arrives already
    /// normalized, so the direction has to travel separately.
    #[allow(clippy::too_many_arguments)]
    #[wasm_bindgen(js_name = roundRect)]
    pub fn round_rect(
        &mut self,
        x: f32,
        y: f32,
        width: f32,
        height: f32,
        radii: &[f32],
        counterclockwise: bool,
    ) -> Result<(), JsValue> {
        let radii = elliptical_radii(radii)?;
        let winding = if counterclockwise {
            Winding::CounterClockwise
        } else {
            Winding::Clockwise
        };
        self.change(|path| {
            path.rrect_radii_elliptical_wound(Rect::new(x, y, width, height), radii, winding);
        });
        Ok(())
    }

    /// `arc` adds a circular arc.
    ///
    /// Angles are radians clockwise from +x. Sweeps are limited to one full
    /// turn. An active contour is connected to the arc's first point.
    pub fn arc(
        &mut self,
        center_x: f32,
        center_y: f32,
        radius: f32,
        start_angle: f32,
        sweep_angle: f32,
    ) {
        self.change(|path| {
            path.arc((center_x, center_y), radius, start_angle, sweep_angle);
        });
    }

    /// `ellipse` adds an elliptical arc.
    ///
    /// `radiusX` and `radiusY` are half-extents, `rotation` turns the ellipse
    /// in radians, and angles are radians clockwise from +x. Sweeps are
    /// limited to one full turn. Non-finite input is ignored. Negative radii
    /// trigger a debug assertion and are ignored in release builds.
    #[allow(clippy::too_many_arguments)]
    pub fn ellipse(
        &mut self,
        center_x: f32,
        center_y: f32,
        radius_x: f32,
        radius_y: f32,
        rotation: f32,
        start_angle: f32,
        sweep_angle: f32,
    ) {
        self.change(|path| {
            path.ellipse(
                (center_x, center_y),
                [radius_x, radius_y],
                rotation,
                start_angle,
                sweep_angle,
            );
        });
    }

    /// `arcTo` rounds the corner between the current point, `(x1, y1)`, and `(x2, y2)`.
    ///
    /// Zero or negative radius, coincident points, and straight-through corners
    /// fall back to a line ending at `(x1, y1)`.
    #[wasm_bindgen(js_name = arcTo)]
    pub fn arc_to(&mut self, x1: f32, y1: f32, x2: f32, y2: f32, radius: f32) {
        self.change(|path| {
            path.arc_to((x1, y1), (x2, y2), radius);
        });
    }

    /// `addPath` appends another path's verbs, optionally through a transform.
    ///
    /// `transform` is 6 affine values, 16 column-major matrix values, or empty
    /// for the identity. Any other length throws. The appended path's final
    /// contour becomes the current contour for subsequent commands.
    #[wasm_bindgen(js_name = addPath)]
    pub fn add_path(&mut self, other: &mut WebPath, transform: &[f32]) -> Result<(), JsValue> {
        let matrix = if transform.is_empty() {
            valo::Matrix::IDENTITY
        } else {
            types::matrix(transform)?
        };
        let appended = other.built();
        self.change(|path| {
            path.append(&appended, &matrix);
        });
        Ok(())
    }

    /// `contains` reports whether `(x, y)` lies inside the filled path.
    ///
    /// The query evaluates the original curves, implicitly closes open contours,
    /// and treats points on the outline as inside. `fillRule` is `0` nonzero
    /// (and any value other than `1`) or `1` even-odd.
    pub fn contains(&mut self, x: f32, y: f32, fill_rule: u32) -> bool {
        self.built()
            .contains(Point::new(x, y), types::fill_rule(fill_rule))
    }

    /// `strokeContains` reports whether `(x, y)` lands on the ink this path would stroke.
    ///
    /// Dashing counts — a gap is not ink. `width` is the full stroke width in
    /// path coordinates. `cap` is `0` butt, `1` round, or `2` square; any other
    /// value uses butt. `join` is `0` miter, `1` round, or `2` bevel; any other
    /// value uses miter. `miterLimit` is the maximum miter length divided by
    /// half the stroke width. An empty `dash` is a solid stroke; otherwise
    /// intervals alternate painted and skipped lengths starting with painted,
    /// and `dashOffset` is the phase into that cycle.
    #[allow(clippy::too_many_arguments)]
    #[wasm_bindgen(js_name = strokeContains)]
    pub fn stroke_contains(
        &mut self,
        x: f32,
        y: f32,
        width: f32,
        cap: u32,
        join: u32,
        miter_limit: f32,
        dash: &[f32],
        dash_offset: f32,
    ) -> bool {
        let stroke = Stroke {
            width,
            cap: types::cap(cap),
            join: types::join(join),
            miter_limit,
            dash: (!dash.is_empty()).then(|| Dash {
                intervals: dash.to_vec(),
                phase: dash_offset,
            }),
        };
        // Hit-testing happens in the path's own space, so the flattening
        // tolerance is the identity transform's.
        let tolerance = valo::local_tolerance(&valo::Matrix::IDENTITY);
        let contours = self.built().flatten(tolerance);
        let contours = match &stroke.dash {
            Some(dash) => valo::dash_contours(&contours, dash),
            None => contours,
        };
        valo::stroke_contains(&contours, &stroke, tolerance, Point::new(x, y))
    }
}

impl Default for WebPath {
    fn default() -> Self {
        Self::new()
    }
}

pub(crate) fn elliptical_radii(values: &[f32]) -> Result<[[f32; 2]; 4], JsValue> {
    match values {
        [radius] => Ok([[*radius; 2]; 4]),
        [top_left, top_right, bottom_right, bottom_left] => Ok([
            [*top_left; 2],
            [*top_right; 2],
            [*bottom_right; 2],
            [*bottom_left; 2],
        ]),
        [tlx, tly, trx, try_, brx, bry, blx, bly] => {
            Ok([[*tlx, *tly], [*trx, *try_], [*brx, *bry], [*blx, *bly]])
        }
        _ => Err(JsValue::from_str("radii need 1, 4, or 8 values")),
    }
}
