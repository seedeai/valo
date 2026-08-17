use crate::{Point, Rect};

/// `Matrix` is a full 4×4 column-major transform.
///
/// Valo maps two-dimensional input as `(x, y, 0, 1)`, including perspective
/// division. The renderer ignores transformed z for draw ordering.
#[derive(Clone, Copy, Debug, PartialEq)]
#[cfg_attr(feature = "serde", derive(serde::Serialize, serde::Deserialize))]
#[cfg_attr(feature = "serde", serde(transparent))]
pub struct Matrix(glam::Mat4);

/// `MatrixKind` classifies the behavior relevant to two-dimensional rendering.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum MatrixKind {
    /// `AxisAligned` contains only positive scale and translation.
    AxisAligned,
    /// `Affine` includes rotation, shear, or reflection without perspective.
    Affine,
    /// `General` includes perspective.
    General,
}

/// `W_EPSILON` keeps points at the eye plane from producing unbounded values.
const W_EPSILON: f32 = 1e-6;

impl Matrix {
    /// `IDENTITY` leaves coordinates unchanged.
    pub const IDENTITY: Matrix = Matrix(glam::Mat4::IDENTITY);

    /// `translation` creates a two-dimensional translation.
    pub fn translation(tx: f32, ty: f32) -> Self {
        Matrix(glam::Mat4::from_translation(glam::Vec3::new(tx, ty, 0.0)))
    }

    /// `scale` creates a two-dimensional scale.
    pub fn scale(sx: f32, sy: f32) -> Self {
        Matrix(glam::Mat4::from_scale(glam::Vec3::new(sx, sy, 1.0)))
    }

    /// `rotation` creates a rotation around the origin.
    ///
    /// Positive angles rotate clockwise in Valo's y-down coordinate system.
    pub fn rotation(radians: f32) -> Self {
        Matrix(glam::Mat4::from_rotation_z(radians))
    }

    /// `from_affine` creates a matrix from `[a, b, c, d, tx, ty]`.
    ///
    /// The linear columns are `(a, b)` and `(c, d)`.
    pub fn from_affine(a: f32, b: f32, c: f32, d: f32, tx: f32, ty: f32) -> Self {
        Matrix(glam::Mat4::from_cols_array(&[
            a, b, 0.0, 0.0, //
            c, d, 0.0, 0.0, //
            0.0, 0.0, 1.0, 0.0, //
            tx, ty, 0.0, 1.0,
        ]))
    }

    /// `from_flutter_array` creates a matrix from 16 column-major Flutter values.
    pub fn from_flutter_array(values: &[f32; 16]) -> Self {
        Matrix(glam::Mat4::from_cols_array(values))
    }

    /// `to_flutter_array` returns 16 column-major Flutter values.
    pub fn to_flutter_array(&self) -> [f32; 16] {
        self.0.to_cols_array()
    }

    /// `to_mat4` returns the backing glam matrix.
    pub fn to_mat4(self) -> glam::Mat4 {
        self.0
    }

    /// `then` composes this matrix with `other`.
    ///
    /// The result applies `other` first and this matrix second.
    pub fn then(&self, other: &Matrix) -> Matrix {
        Matrix(self.0 * other.0)
    }

    /// `is_affine` reports whether two-dimensional input has no perspective.
    pub fn is_affine(&self) -> bool {
        let m = &self.0;
        m.x_axis.w == 0.0 && m.y_axis.w == 0.0 && m.w_axis.w == 1.0
    }

    /// `kind` classifies this matrix for two-dimensional rendering.
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

    /// `map_point` transforms a point and applies perspective division.
    ///
    /// Points at or behind the eye plane use a small positive divisor.
    pub fn map_point(&self, p: Point) -> Point {
        let v = self.0 * glam::Vec4::new(p.x, p.y, 0.0, 1.0);
        let w = if v.w > W_EPSILON { v.w } else { W_EPSILON };
        Point::new(v.x / w, v.y / w)
    }

    /// `map_rect` returns axis-aligned bounds around a transformed rectangle.
    ///
    /// It returns [`Rect::EVERYTHING`] when a corner reaches or crosses the
    /// eye plane and finite conservative bounds cannot be proven.
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

    /// `max_scale` returns the larger length of the transformed x and y basis vectors.
    ///
    /// It ignores perspective and is therefore approximate for general matrices.
    pub fn max_scale(&self) -> f32 {
        let m = &self.0;
        let sx = (m.x_axis.x * m.x_axis.x + m.x_axis.y * m.x_axis.y).sqrt();
        let sy = (m.y_axis.x * m.y_axis.x + m.y_axis.y * m.y_axis.y).sqrt();
        sx.max(sy)
    }

    /// `to_affine` returns `[a, b, c, d, tx, ty]` from the two-dimensional block.
    ///
    /// Perspective components are omitted; check [`Self::is_affine`] when
    /// exact conversion is required.
    pub fn to_affine(&self) -> [f32; 6] {
        let m = &self.0;
        [
            m.x_axis.x, m.x_axis.y, m.y_axis.x, m.y_axis.y, m.w_axis.x, m.w_axis.y,
        ]
    }

    /// `determinant` returns the signed area scale of the two-dimensional block.
    pub fn determinant(&self) -> f32 {
        let m = &self.0;
        m.x_axis.x * m.y_axis.y - m.x_axis.y * m.y_axis.x
    }

    /// `invert` returns the inverse matrix or `None` when no finite inverse exists.
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
