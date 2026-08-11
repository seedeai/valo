//! Pure 2D math for valo: points, rects, affine transforms, color.
//! Zero dependencies (serde optional) — usable anywhere, no GPU, no unicode.
//!
//! Conventions (fixed across all of valo):
//! - y-down, origin top-left, units are logical pixels until a transform says otherwise.
//! - The public transform is a full 4×4 (glam-backed); the renderer folds depth in (
//!   is a renderer concern — clips consume z, the user never sees it).
//! - `Color` is straight (unpremultiplied) sRGB; premultiplication happens at the GPU
//!   boundary. Blending is performed in sRGB space (the CSS/Skia-compatible look) —
//!   linear/wide-gamut blending is deliberately deferred.

mod color;
mod matrix;
mod measure;
mod path;
mod point;
mod rect;
mod stroke;
mod winding;

pub use color::Color;
pub use matrix::{Matrix, MatrixKind};
pub use measure::{ContourMeasure, PathSample};
pub use path::{
    constrain_radii, constrain_radii_elliptical, local_tolerance, Contour, FillRule, Path,
    PathBuilder,
};
pub use point::{Point, Size};
pub use rect::Rect;
pub use stroke::{dash_contours, stroke_strip, Cap, Dash, Join, Stroke};
