//! System-installed fonts over the C ABI: scan once, then
//! answer font demands — the whole loop in one call, or by family for
//! pre-registration. Faces register into a [`ValoFontCollection`] exactly
//! like embedder-supplied bytes, so nothing downstream can tell them apart.

use std::sync::Arc;

use valo::FontSource;
use valo_system_fonts::SystemFonts;

use crate::text::utf8;
use crate::{borrow, borrow_mut, dispose_handle, into_handle, ValoFontCollection, ValoParagraph};

pub struct ValoSystemFonts {
    fonts: SystemFonts,
}

/// Scan the platform's font directories — the expensive step, so keep the
/// handle (creating it lazily on the first demand is the intended
/// pattern). Check [`valo_system_fonts_face_count`] for an empty scan.
#[no_mangle]
pub extern "C" fn valo_system_fonts_new() -> *mut ValoSystemFonts {
    into_handle(ValoSystemFonts {
        fonts: SystemFonts::load(),
    })
}

/// # Safety
/// `system_fonts` must be a live [`valo_system_fonts_new`] handle (or null).
#[no_mangle]
pub unsafe extern "C" fn valo_system_fonts_dispose(system_fonts: *mut ValoSystemFonts) {
    unsafe { dispose_handle(system_fonts) }
}

/// Installed faces the scan found (0 = nothing to answer with).
///
/// # Safety
/// `system_fonts` must be a live handle (or null → 0).
#[no_mangle]
pub unsafe extern "C" fn valo_system_fonts_face_count(
    system_fonts: *const ValoSystemFonts,
) -> usize {
    unsafe { borrow(system_fonts) }.map_or(0, |handle| handle.fonts.face_count())
}

/// Register every installed face of the named family (all weights and
/// styles — nearest-variant matching picks per span). Returns the number
/// of faces added; 0 when the family is not installed.
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
    let mut next = (*fonts.collection).clone();
    for face in faces {
        next.add(face);
    }
    fonts.collection = Arc::new(next);
    count
}

/// Answer a paragraph's font demand from the installed fonts: missing
/// families register under their own names, still-uncovered codepoints
/// extend the fallback chain. True when the collection grew — then
/// re-register it with the context, rebuild the paragraph, and lay out
/// again (the demand loop).
///
/// # Safety
/// All three must be live handles (or null → false).
#[no_mangle]
pub unsafe extern "C" fn valo_fonts_satisfy_demand(
    fonts: *mut ValoFontCollection,
    system_fonts: *mut ValoSystemFonts,
    paragraph: *const ValoParagraph,
) -> bool {
    let (Some(fonts), Some(system), Some(paragraph)) = (
        unsafe { borrow_mut(fonts) },
        unsafe { borrow_mut(system_fonts) },
        unsafe { borrow(paragraph) },
    ) else {
        return false;
    };
    let demand = paragraph.paragraph.demand();
    let Some(next) = system.fonts.satisfy(&fonts.collection, demand) else {
        return false;
    };
    fonts.collection = Arc::new(next);
    true
}
