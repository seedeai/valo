//! The C ABI over valo: a small, stable surface for embedding the renderer
//! from any language that speaks C. The committed header is
//! `include/valo.h` — kept by hand (the API is designed-stable) and
//! guarded by a symbol-parity test.
//!
//! Conventions, uniform across the whole surface:
//! - Every object is an OPAQUE HANDLE created by a `valo_*_new`-style
//!   function and released by the matching `valo_*_dispose`. Handles are
//!   never shared across threads.
//! - Every function is NULL-SAFE: a null handle is a no-op (or returns the
//!   zero value). A C embedder mis-sequencing teardown must not crash.
//! - Geometry and paint travel BY VALUE as `#[repr(C)]` structs — no
//!   retained paint objects, matching valo's paint-per-op display lists.
//! - Strings are UTF-8 (pointer, byte length) — never NUL-terminated.
//! - Angles are radians; colors are straight-alpha floats in [0, 1].

mod builder;
mod color_filter;
mod context;
mod path;
mod system_fonts;
mod text;
mod types;

pub use builder::*;
pub use color_filter::*;
pub use context::*;
pub use path::*;
pub use system_fonts::*;
pub use text::*;
pub use types::*;

/// Box a value into an opaque pointer the C side owns.
pub(crate) fn into_handle<T>(value: T) -> *mut T {
    Box::into_raw(Box::new(value))
}

/// Reclaim and drop a handle; null is a no-op.
///
/// # Safety
/// `handle` must be null or a pointer produced by [`into_handle`] that has
/// not been disposed already.
pub(crate) unsafe fn dispose_handle<T>(handle: *mut T) {
    if !handle.is_null() {
        drop(unsafe { Box::from_raw(handle) });
    }
}

/// Borrow a handle; None when null.
pub(crate) unsafe fn borrow<'a, T>(handle: *const T) -> Option<&'a T> {
    unsafe { handle.as_ref() }
}

/// Mutably borrow a handle; None when null.
pub(crate) unsafe fn borrow_mut<'a, T>(handle: *mut T) -> Option<&'a mut T> {
    unsafe { handle.as_mut() }
}
