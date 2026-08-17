//! System-installed fonts over the C ABI: scan once, then
//! answer font demands — the whole loop in one call, or by family for
//! pre-registration. Faces register into a [`ValoFontCollection`] exactly
//! like embedder-supplied bytes, so nothing downstream can tell them apart.

use valo::FontSource;
use valo_system_fonts::SystemFonts;

use crate::text::utf8;
use crate::{borrow, borrow_mut, dispose_handle, into_handle, ValoFontCollection};

/// `ValoSystemFonts` is a scan of the platform's installed fonts.
///
/// Create it with [`valo_system_fonts_new`] (the expensive directory walk —
/// keep the handle; creating it lazily on the first demand is the intended
/// pattern). Use [`valo_fonts_satisfy_demand`] or
/// [`valo_fonts_add_system_family`] to register faces into a
/// [`ValoFontCollection`]. Dispose with [`valo_system_fonts_dispose`].
/// Not available as a wasm dependency.
pub struct ValoSystemFonts {
    fonts: SystemFonts,
}

/// `valo_system_fonts_new` scans the platform's font directories.
///
/// This is the expensive step, so keep the handle (creating it lazily on
/// the first demand is the intended pattern). Check
/// [`valo_system_fonts_face_count`] for an empty scan.
#[no_mangle]
pub extern "C" fn valo_system_fonts_new() -> *mut ValoSystemFonts {
    into_handle(ValoSystemFonts {
        fonts: SystemFonts::load(),
    })
}

/// `valo_system_fonts_dispose` releases a system-fonts handle. Null is a no-op.
///
/// # Safety
/// `system_fonts` must be a live [`valo_system_fonts_new`] handle (or null).
#[no_mangle]
pub unsafe extern "C" fn valo_system_fonts_dispose(system_fonts: *mut ValoSystemFonts) {
    unsafe { dispose_handle(system_fonts) }
}

/// `valo_system_fonts_face_count` returns how many installed faces the scan found.
///
/// Zero means nothing to answer with. Null handle returns 0.
///
/// # Safety
/// `system_fonts` must be a live handle (or null → 0).
#[no_mangle]
pub unsafe extern "C" fn valo_system_fonts_face_count(
    system_fonts: *const ValoSystemFonts,
) -> usize {
    unsafe { borrow(system_fonts) }.map_or(0, |handle| handle.fonts.face_count())
}

/// `valo_fonts_add_system_family` registers every installed face of the named family.
///
/// All weights and styles are added — nearest-variant matching picks per
/// span. Returns the number of faces added; 0 when the family is not
/// installed, the name is not valid UTF-8, or a handle is null.
///
/// # Safety
/// `fonts` and `system_fonts` must be live handles; `name_utf8` must point
/// to `name_length` readable bytes of UTF-8.
#[no_mangle]
pub unsafe extern "C" fn valo_fonts_add_system_family(
    fonts: *mut ValoFontCollection,
    system_fonts: *mut ValoSystemFonts,
    name_utf8: *const u8,
    name_length: usize,
) -> i32 {
    let (Some(fonts), Some(system)) = (unsafe { borrow_mut(fonts) }, unsafe {
        borrow_mut(system_fonts)
    }) else {
        return 0;
    };
    let Some(name) = (unsafe { utf8(name_utf8, name_length) }) else {
        return 0;
    };
    let faces = system.fonts.family(name);
    if faces.is_empty() {
        return 0;
    }
    let count = faces.len() as i32;
    for face in faces {
        fonts.collection.add(face);
    }
    count
}

/// `valo_fonts_satisfy_demand` answers unanswered font requests from installed fonts.
///
/// Missing families register under their own names; still-uncovered
/// codepoints extend the fallback chain. Returns true when the collection
/// grew — rebuild the affected paragraphs to pick it up. Hosts that install
/// a source on the collection need none of this: resolution happens during
/// [`crate::valo_paragraph_builder_build`].
///
/// # Safety
/// Both must be live handles (or null → false).
#[no_mangle]
pub unsafe extern "C" fn valo_fonts_satisfy_demand(
    fonts: *mut ValoFontCollection,
    system_fonts: *mut ValoSystemFonts,
) -> bool {
    let (Some(fonts), Some(system)) = (unsafe { borrow_mut(fonts) }, unsafe {
        borrow_mut(system_fonts)
    }) else {
        return false;
    };
    let demand = fonts.collection.take_unanswered();
    if demand.is_empty() {
        return false;
    }
    let Some(next) = system.fonts.satisfy(fonts.collection.faces(), &demand) else {
        return false;
    };
    fonts.collection.adopt_faces(next);
    true
}
