//! The parts of Skia's core (`include/core`, `src/core`, `src/base`) that PathOps calls.
//!
//! Each module ports only the members PathOps, or the core functions here, reach.

pub(crate) mod float_bits;
pub(crate) mod geometry;
pub(crate) mod path;
pub(crate) mod path_builder;
pub(crate) mod point;
pub(crate) mod rect;
pub(crate) mod t_sort;
