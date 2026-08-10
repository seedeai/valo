//! Colour filter handles. A filter is small and immutable, so the handle
//! exists only to keep [`crate::ValoPaint`] — copied for every draw — at eight
//! bytes instead of carrying a 4×5 matrix inline.

use valo::ColorFilter;

use crate::{blend_mode, dispose_handle, into_handle, ValoColor};

pub struct ValoColorFilter(ColorFilter);

/// Build a 4×5 colour matrix filter from 20 row-major floats over
/// UNPREMULTIPLIED colour in 0..1.
///
/// Flutter's `ColorFilter.matrix` gives the translation column (entries 4, 9,
/// 14 and 19) in unnormalized 0..255 space instead — divide those four by 255
/// before calling, or every offset comes out 255× too strong.
///
/// # Safety
/// `matrix` must point at 20 readable floats (null returns null).
#[no_mangle]
pub unsafe extern "C" fn valo_color_filter_matrix(matrix: *const f32) -> *mut ValoColorFilter {
    if matrix.is_null() {
        return std::ptr::null_mut();
    }
    let mut rows = [0.0f32; 20];
    rows.copy_from_slice(unsafe { std::slice::from_raw_parts(matrix, 20) });
    into_handle(ValoColorFilter(ColorFilter::Matrix(rows)))
}

/// Blend `color` AS THE SOURCE over what was drawn — Flutter's
/// `ColorFilter.mode`. `mode` indexes the same 29 blend modes as
/// [`crate::ValoPaint::blend_mode`].
#[no_mangle]
pub extern "C" fn valo_color_filter_blend(color: ValoColor, mode: i32) -> *mut ValoColorFilter {
    into_handle(ValoColorFilter(ColorFilter::Blend(
        color.into(),
        blend_mode(mode),
    )))
}

/// # Safety
/// `filter` must be a live handle (or null).
#[no_mangle]
pub unsafe extern "C" fn valo_color_filter_dispose(filter: *mut ValoColorFilter) {
    unsafe { dispose_handle(filter) }
}

/// The filter a paint borrows, if any.
///
/// # Safety
/// `filter` must be null or a live handle.
pub(crate) unsafe fn color_filter_of(filter: *const ValoColorFilter) -> Option<ColorFilter> {
    unsafe { filter.as_ref() }.map(|f| f.0)
}
