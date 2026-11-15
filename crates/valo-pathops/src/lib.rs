//! Skia's PathOps: boolean operations on paths, ported line for line.
//!
//! [`op`] combines two paths as Skia's `Op` does, which Flutter's
//! `Path.combine` calls under both Skia and Impeller: it finds every
//! intersection between the two paths' curves, works out which side of each
//! piece is inside the result, and writes the pieces that bound it as a new
//! path filled with the even-odd rule. Skia keeps the work in doubles,
//! handles lines, quadratics, conics and cubics without flattening them, and
//! gives up on input it cannot resolve; [`op`] returns an error exactly
//! where Skia's `Op` returns false.
//!
//! Each module is one of Skia's `src/pathops` files, header and source
//! together, under the same name, and keeps Skia's type and function names
//! so the two read side by side. `core` holds the parts of Skia's core
//! (`SkPath`, `SkGeometry`, `SkTSort`, ...) that PathOps calls.
//!
//! Skia's objects point at each other (spans at segments, angles at spans,
//! linked lists and rings throughout). Here each such object is a copyable
//! handle named after Skia's class, standing for Skia's pointer, and its
//! fields live in `SkOpGlobalState`, which every operation is handed, as
//! Skia hands it its global state.
#![forbid(unsafe_code)]
#![allow(clippy::too_many_arguments)]

mod core;

mod add_intersections;
mod d_conic_line_intersection;
mod d_cubic_line_intersection;
mod d_line_intersection;
mod d_quad_line_intersection;
mod intersection_helper;
mod intersections;
mod line_parameters;
mod op_angle;
mod op_coincidence;
mod op_contour;
mod op_cubic_hull;
mod op_edge_builder;
mod op_segment;
mod op_span;
mod path_ops_bounds;
mod path_ops_common;
mod path_ops_conic;
mod path_ops_cubic;
mod path_ops_curve;
mod path_ops_line;
mod path_ops_op;
mod path_ops_point;
mod path_ops_quad;
mod path_ops_rect;
mod path_ops_simplify;
mod path_ops_t_curve;
mod path_ops_t_sect;
mod path_ops_types;
mod path_ops_winding;
mod path_writer;
mod reduce_order;

pub use crate::core::path::{SkPath, SkPathFillType, SkPathVerb};
pub use crate::core::path_builder::SkPathBuilder;
pub use crate::core::point::SkPoint;
pub use path_ops_op::op;
pub use path_ops_simplify::simplify;
pub use path_ops_types::{PathOpsError, SkPathOp};
