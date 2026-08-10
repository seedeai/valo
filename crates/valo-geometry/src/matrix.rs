use crate::{Point, Rect};

/// Full 4×4 column-major transform (glam-backed — the byte layout Flutter
/// and Impeller use).
///
/// Canvas semantics throughout valo: the current transform maps LOCAL
/// (drawn) coordinates to the list's ROOT space, and `then(local)` appends
/// a transform that applies to subsequently drawn geometry FIRST — i.e.
/// `current ∘ local`, matrix product `current × local`. Same convention as
/// Skia/Impeller's transform stack.
///
/// 2D content maps as (x, y, 0, 1): the w row does perspective (the
/// hardware divide, with perspective-correct interpolation); the z output
/// is IGNORED for painting — valo writes its own per-draw depth (2.5D,
/// Flutter's model).
#[derive(Clone, Copy, Debug, PartialEq)]
#[cfg_attr(feature = "serde", derive(serde::Serialize, serde::Deserialize))]
#[cfg_attr(feature = "serde", serde(transparent))]
pub struct Matrix(glam::Mat4);

/// What the fast paths may assume about a matrix — computed when the
/// transform stack changes, consulted per draw.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum MatrixKind {
    /// Positive scale + translation only: device-snapped glyphs, scissor
    /// clips, and analytic blur stay exact.
    AxisAligned,
    /// Any other affine (rotation, shear, flips).
    Affine,
    /// A live perspective row: conservative bounds, approximate scales.
    General,
}

/// Below this w a projected corner counts as at/behind the eye plane —
/// bounds go conservative instead of exploding across the flip.
const W_EPSILON: f32 = 1e-6;

impl Matrix {
    pub const IDENTITY: Matrix = Matrix(glam::Mat4::IDENTITY);

    pub fn translation(tx: f32, ty: f32) -> Self {
        Matrix(glam::Mat4::from_translation(glam::Vec3::new(tx, ty, 0.0)))
    }

    pub fn scale(sx: f32, sy: f32) -> Self {
        Matrix(glam::Mat4::from_scale(glam::Vec3::new(sx, sy, 1.0)))
    }

    pub fn rotation(radians: f32) -> Self {
        Matrix(glam::Mat4::from_rotation_z(radians))
    }

    /// The classic 2×3 affine (column vectors (a,b), (c,d), translation).
    pub fn from_affine(a: f32, b: f32, c: f32, d: f32, tx: f32, ty: f32) -> Self {
        Matrix(glam::Mat4::from_cols_array(&[
            a, b, 0.0, 0.0, //
            c, d, 0.0, 0.0, //
            0.0, 0.0, 1.0, 0.0, //
            tx, ty, 0.0, 1.0,
        ]))
    }

    /// The 16 column-major floats Flutter-architecture hosts hand a canvas.
    pub fn from_flutter_array(values: &[f32; 16]) -> Self {
        Matrix(glam::Mat4::from_cols_array(values))
    }

    pub fn to_flutter_array(&self) -> [f32; 16] {
        self.0.to_cols_array()
    }

    /// The backing matrix, for MVP assembly.
    pub fn to_mat4(self) -> glam::Mat4 {
        self.0
    }

    /// `self ∘ other`: apply `other` first, then `self` (product self × other).
    pub fn then(&self, other: &Matrix) -> Matrix {
        Matrix(self.0 * other.0)
    }

    /// True when the w row is inert for 2D content ((0, 0, ·, 1) — the
    /// z column never matters because inputs have z = 0).
    pub fn is_affine(&self) -> bool {
        let m = &self.0;
        m.x_axis.w == 0.0 && m.y_axis.w == 0.0 && m.w_axis.w == 1.0
    }

    pub fn kind(&self) -> MatrixKind {
        if !self.is_affine() {
            return MatrixKind::General;
        }
        let m = &self.0;
        let axis_aligned =
            m.x_axis.y == 0.0 && m.y_axis.x == 0.0 && m.x_axis.x > 0.0 && m.y_axis.y > 0.0;
        if axis_aligned {
            MatrixKind::AxisAligned
        } else {
            MatrixKind::Affine
        }
    }

    pub fn map_point(&self, p: Point) -> Point {
        let v = self.0 * glam::Vec4::new(p.x, p.y, 0.0, 1.0);
        let w = if v.w > W_EPSILON { v.w } else { W_EPSILON };
        Point::new(v.x / w, v.y / w)
    }

    /// Axis-aligned bounds of the mapped rect: exact for rectilinear
    /// transforms, conservative under rotation, and [`Rect::EVERYTHING`]
    /// when any corner reaches the eye plane (w ≤ ε) — culling must never
    /// reject such content, and layers clamp to their clip instead.
    pub fn map_rect(&self, r: &Rect) -> Rect {
        let (mut left, mut top) = (f32::MAX, f32::MAX);
        let (mut right, mut bottom) = (f32::MIN, f32::MIN);
        for corner in r.corners() {
            let v = self.0 * glam::Vec4::new(corner.x, corner.y, 0.0, 1.0);
            if v.w <= W_EPSILON {
                return Rect::EVERYTHING;
            }
            let (x, y) = (v.x / v.w, v.y / v.w);
            left = left.min(x);
            top = top.min(y);
            right = right.max(x);
            bottom = bottom.max(y);
        }
        Rect::from_ltrb(left, top, right, bottom)
    }

    /// Maximum length the XY basis vectors scale a unit vector to — the
    /// device-scale factor text pickers and blur sigmas care about
    /// (Impeller's maxBasisLengthXY; ignores perspective, so approximate
    /// under it).
    pub fn max_scale(&self) -> f32 {
        let m = &self.0;
        let sx = (m.x_axis.x * m.x_axis.x + m.x_axis.y * m.x_axis.y).sqrt();
        let sy = (m.y_axis.x * m.y_axis.x + m.y_axis.y * m.y_axis.y).sqrt();
        sx.max(sy)
    }

    /// The 2D affine block `[a, b, c, d, tx, ty]` (column vectors (a,b),
    /// (c,d), translation) — ignores any perspective row; pair with
    /// [`Self::is_affine`] where exactness matters (gradient locals and
    /// embed quads are affine by construction).
    pub fn to_affine(&self) -> [f32; 6] {
        let m = &self.0;
        [
            m.x_axis.x, m.x_axis.y, m.y_axis.x, m.y_axis.y, m.w_axis.x, m.w_axis.y,
        ]
    }

    /// The 2D block's determinant (orientation / area factor of the
    /// xy plane — what stroking and winding care about).
    pub fn determinant(&self) -> f32 {
        let m = &self.0;
        m.x_axis.x * m.y_axis.y - m.x_axis.y * m.y_axis.x
    }

    pub fn invert(&self) -> Option<Matrix> {
        let det = self.0.determinant();
        if det == 0.0 || !det.is_finite() {
            return None;
        }
        let inverse = self.0.inverse();
        inverse.is_finite().then_some(Matrix(inverse))
    }
}

impl Default for Matrix {
    fn default() -> Self {
        Self::IDENTITY
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn close(a: Point, b: Point) -> bool {
        (a.x - b.x).abs() < 1e-4 && (a.y - b.y).abs() < 1e-4
    }

    #[test]
    fn near_singular_matrices_invert_to_none() {
        assert!(Matrix::scale(1e-20, 1e-20).invert().is_none());
        assert!(Matrix::scale(0.0, 1.0).invert().is_none());
    }

    #[test]
    fn then_applies_local_first() {
        // translate then scale: point scales, THEN translates (current ∘ local).
        let t = Matrix::translation(10.0, 0.0).then(&Matrix::scale(2.0, 2.0));
        assert!(close(
            t.map_point(Point::new(1.0, 1.0)),
            Point::new(12.0, 2.0)
        ));
    }

    #[test]
    fn rotation_quarter_turn() {
        let t = Matrix::rotation(std::f32::consts::FRAC_PI_2);
        // y-down: (1,0) rotates clockwise to (0,1).
        assert!(close(
            t.map_point(Point::new(1.0, 0.0)),
            Point::new(0.0, 1.0)
        ));
    }

    #[test]
    fn invert_roundtrip() {
        let t = Matrix::translation(5.0, -3.0)
            .then(&Matrix::rotation(0.7))
            .then(&Matrix::scale(2.0, 0.5));
        let inv = t.invert().unwrap();
        let p = Point::new(3.0, 4.0);
        assert!(close(inv.map_point(t.map_point(p)), p));
    }

    #[test]
    fn map_rect_rotation_is_conservative_bounds() {
        let t = Matrix::rotation(std::f32::consts::FRAC_PI_4);
        let r = t.map_rect(&Rect::new(-1.0, -1.0, 2.0, 2.0));
        let d = 2.0_f32.sqrt();
        assert!((r.width - 2.0 * d).abs() < 1e-4 && (r.height - 2.0 * d).abs() < 1e-4);
    }

    #[test]
    fn perspective_divides_by_w() {
        // Flutter's classic card tilt: entry[3][2] bends z into w — for 2D
        // content that only matters through concatenation (below); a raw
        // w-row on x makes near points larger than far ones.
        let mut values = Matrix::IDENTITY.to_flutter_array();
        values[3] = 0.001; // w += 0.001 · x
        let t = Matrix::from_flutter_array(&values);
        assert!(close(
            t.map_point(Point::new(100.0, 100.0)),
            Point::new(100.0 / 1.1, 100.0 / 1.1)
        ));
        assert_eq!(t.kind(), MatrixKind::General);
    }

    #[test]
    fn concatenation_stays_four_by_four() {
        // tilt ∘ translate ∘ tilt: the sequence Flutter's Transform widgets
        // produce. Slicing each factor to its 2D action BEFORE multiplying
        // loses the z column the middle translation feeds into the outer
        // tilt's w row — full 4×4 concatenation keeps it.
        let mut tilt_values = Matrix::IDENTITY.to_flutter_array();
        tilt_values[11] = 0.001; // w += 0.001 · z (the Flutter entry(3,2))
        let tilt = Matrix::from_flutter_array(&tilt_values);
        let mut rotate_x = Matrix::IDENTITY.to_flutter_array();
        // rotateX(0.5): y/z plane rotation — feeds y into z.
        let (sin, cos) = 0.5_f32.sin_cos();
        rotate_x[5] = cos;
        rotate_x[6] = sin;
        rotate_x[9] = -sin;
        rotate_x[10] = cos;
        let full = tilt.then(&Matrix::from_flutter_array(&rotate_x));
        // The composed matrix must carry perspective from y (via z).
        assert_eq!(full.kind(), MatrixKind::General);
        let p = full.map_point(Point::new(0.0, 100.0));
        // y rotated toward the viewer shrinks: 100·cos / (1 + 0.001·100·sin).
        let expected_y = 100.0 * cos / (1.0 + 0.001 * 100.0 * sin);
        assert!((p.y - expected_y).abs() < 1e-2, "{} vs {expected_y}", p.y);
    }

    #[test]
    fn eye_plane_bounds_are_everything() {
        let mut values = Matrix::IDENTITY.to_flutter_array();
        values[3] = -0.1; // w = 1 - 0.1 · x → w ≤ 0 from x = 10 on
        let t = Matrix::from_flutter_array(&values);
        assert_eq!(
            t.map_rect(&Rect::new(0.0, 0.0, 100.0, 10.0)),
            Rect::EVERYTHING
        );
    }
}
