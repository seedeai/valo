//! GPU-free geometry and color types for Valo.
//!
//! Coordinates are y-down with the origin at the top-left. Units are logical
//! pixels until transformed. [`Matrix`] is a full 4×4 transform, while draw
//! ordering remains a renderer concern. [`Color`] stores straight-alpha sRGB.

mod color;
mod matrix;
mod measure;
mod path;
mod point;
mod rect;
mod stroke;
mod superellipse;
mod winding;

pub use color::Color;
pub use matrix::{Matrix, MatrixKind};
pub use measure::{ContourMeasure, PathSample};
pub use path::{
    Contour, FillRule, Path, PathBuilder, PathElement, Winding, constrain_radii,
    constrain_radii_elliptical, local_tolerance,
};
pub use point::{Point, Size};
pub use rect::Rect;
pub use stroke::{Cap, Dash, Join, Stroke, dash_contours, stroke_contains, stroke_strip};
pub use superellipse::RoundSuperellipse;
