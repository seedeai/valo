//! CoreText as the font manager, on macOS and iOS.
//!
//! CoreText answers every question itself: families by name, fallback for a character in
//! a language (`CTFontCreateForStringWithLanguage`, the cascade the system's own text
//! uses), and the user-interface font (`CTFontCreateUIFontForLanguage`). What it hands
//! back is a font by name and file; the face's index in a collection file comes from
//! matching its PostScript name against the file, as Skia's CoreText backend does.

use std::ffi::c_void;
use std::path::Path;

use core_foundation::array::CFArray;
use core_foundation::base::CFType;
use core_foundation::base::{CFRange, TCFType};
use core_foundation::dictionary::CFDictionary;
use core_foundation::string::{CFString, CFStringRef};
use core_foundation_sys::notification_center::{
    CFNotificationCallback, CFNotificationCenterAddObserver, CFNotificationCenterGetLocalCenter,
    CFNotificationCenterRef, CFNotificationCenterRemoveObserver,
    CFNotificationSuspensionBehaviorDeliverImmediately,
};
use core_text::font::{self, CTFont, CTFontRef, kCTFontSystemFontType};
use core_text::font_collection;
use core_text::font_descriptor::{
    self, CTFontDescriptor, CTFontTraits, TraitAccessors, kCTFontItalicTrait,
};
use core_text::font_manager;

use crate::files::{self, Files};
use crate::{FontManager, Slant, Style, Typeface, Watch, nearest};

#[link(name = "CoreText", kind = "framework")]
extern "C" {
    fn CTFontCreateForStringWithLanguage(
        current_font: CTFontRef,
        string: CFStringRef,
        range: CFRange,
        language: CFStringRef,
    ) -> CTFontRef;
    static kCTFontManagerRegisteredFontsChangedNotification: CFStringRef;
}

/// The point size a lookup that has no size of its own is made at; CoreText needs one.
const LOOKUP_SIZE: f64 = 12.0;

/// `CoreText` is the platform's font manager on macOS and iOS.
///
/// Nothing is scanned up front: each answer is one CoreText query, and a font file is
/// read the first time one of its faces is answered.
#[derive(Default)]
pub struct CoreText {
    files: Files,
}

impl CoreText {
    pub fn new() -> CoreText {
        CoreText::default()
    }

    fn typeface_of_descriptor(&mut self, descriptor: &CTFontDescriptor) -> Option<Typeface> {
        self.typeface(
            &descriptor.font_path()?,
            &descriptor.font_name(),
            descriptor.family_name(),
            style_of(&descriptor.traits()),
        )
    }

    fn typeface_of_font(&mut self, font: &CTFont) -> Option<Typeface> {
        self.typeface(
            &font.url()?.to_path()?,
            &font.postscript_name(),
            font.family_name(),
            style_of(&font.all_traits()),
        )
    }

    fn typeface(
        &mut self,
        path: &Path,
        postscript_name: &str,
        family: String,
        style: Style,
    ) -> Option<Typeface> {
        let data = self.files.read(path)?;
        let index = files::face_index((*data).as_ref(), postscript_name)?;
        Some(Typeface::new(data, index, family, style))
    }
}

impl FontManager for CoreText {
    fn family(&mut self, family: &str) -> Vec<Typeface> {
        let Some(descriptors) =
            font_collection::create_for_family(family).and_then(|faces| faces.get_descriptors())
        else {
            return Vec::new();
        };
        descriptors
            .iter()
            .filter_map(|descriptor| self.typeface_of_descriptor(&descriptor))
            .collect()
    }

    fn match_character(
        &mut self,
        family: Option<&str>,
        style: Style,
        locales: &[&str],
        character: char,
    ) -> Option<Typeface> {
        // The base font decides the cascade: a family's own, else the platform's default
        // font, as Skia's backend starts from a descriptor naming no family. The
        // user-interface font would cascade into the platform's reserved UI variants.
        let base = family
            .and_then(|name| font::new_from_name(name, LOOKUP_SIZE).ok())
            .unwrap_or_else(default_font);
        let text = CFString::new(&character.to_string());
        let range = CFRange::init(0, character.len_utf16() as isize);
        let language = locales.first().map(|locale| CFString::new(locale));
        // SAFETY: plain CoreText calls; every argument lives for the call, and the result
        // is owned by the create rule.
        let matched = unsafe {
            let matched = CTFontCreateForStringWithLanguage(
                base.as_concrete_TypeRef(),
                text.as_concrete_TypeRef(),
                range,
                language
                    .as_ref()
                    .map_or(std::ptr::null(), |language| language.as_concrete_TypeRef()),
            );
            if matched.is_null() {
                return None;
            }
            CTFont::wrap_under_create_rule(matched)
        };
        let candidate = self.typeface_of_font(&matched)?;
        // CoreText answers with a last-resort font when nothing covers the character.
        candidate.covers(character).then(|| {
            let mut faces = self.family(candidate.family());
            if faces.is_empty() {
                faces.push(candidate.clone());
            }
            nearest(faces, style).unwrap_or(candidate)
        })
    }

    fn system_font(&mut self, size: f32, style: Style) -> Option<Typeface> {
        let ui_font = font::new_ui_font_for_language(kCTFontSystemFontType, f64::from(size), None);
        let faces = self.family(&ui_font.family_name());
        match nearest(faces, style) {
            Some(face) => Some(face),
            None => self.typeface_of_font(&ui_font),
        }
    }

    fn families(&self) -> Vec<String> {
        font_manager::copy_available_font_family_names()
            .iter()
            .map(|name| name.to_string())
            .collect()
    }

    fn face_count(&self) -> usize {
        // SAFETY: a plain CoreText call whose result is owned by the create rule.
        let names: CFArray<CFString> = unsafe {
            CFArray::wrap_under_create_rule(
                font_manager::CTFontManagerCopyAvailablePostScriptNames(),
            )
        };
        names.len() as usize
    }

    fn watch(&mut self, on_change: Box<dyn Fn() + Send + Sync>) -> Option<Watch> {
        Some(Watch::new(Observer::new(on_change)))
    }
}

/// The platform's default font: what an empty descriptor resolves to.
fn default_font() -> CTFont {
    let no_attributes: CFDictionary<CFString, CFType> = CFDictionary::from_CFType_pairs(&[]);
    font::new_from_descriptor(
        &font_descriptor::new_from_attributes(&no_attributes),
        LOOKUP_SIZE,
    )
}

/// CoreText's normalized weight, -1 to 1, against CSS weights: Skia's table for its
/// CoreText backend, interpolated between entries.
const WEIGHTS: [(f64, f64); 11] = [
    (-1.0, 0.0),
    (-0.8, 100.0),
    (-0.6, 200.0),
    (-0.4, 300.0),
    (0.0, 400.0),
    (0.23, 500.0),
    (0.3, 600.0),
    (0.4, 700.0),
    (0.56, 800.0),
    (0.62, 900.0),
    (1.0, 1000.0),
];

/// CoreText's normalized width, -1 to 1, is nine steps around normal, the `usWidthClass`
/// steps; these are their CSS percentages.
const WIDTHS: [f32; 9] = [50.0, 62.5, 75.0, 87.5, 100.0, 112.5, 125.0, 150.0, 200.0];

fn style_of(traits: &CTFontTraits) -> Style {
    let slant = if traits.symbolic_traits() & kCTFontItalicTrait != 0 {
        Slant::Italic
    } else if traits.normalized_slant() != 0.0 {
        Slant::Oblique
    } else {
        Slant::Upright
    };
    Style {
        weight: css_weight(traits.normalized_weight()),
        width: css_width(traits.normalized_width()),
        slant,
    }
}

fn css_weight(normalized: f64) -> u16 {
    let normalized = normalized.clamp(-1.0, 1.0);
    let upper = WEIGHTS
        .iter()
        .position(|&(at, _)| at >= normalized)
        .unwrap_or(WEIGHTS.len() - 1);
    if upper == 0 {
        return WEIGHTS[0].1 as u16;
    }
    let (from, from_weight) = WEIGHTS[upper - 1];
    let (to, to_weight) = WEIGHTS[upper];
    let fraction = (normalized - from) / (to - from);
    (from_weight + fraction * (to_weight - from_weight)).round() as u16
}

fn css_width(normalized: f64) -> f32 {
    let step = ((normalized * 4.0).round() as i32 + 4).clamp(0, 8);
    WIDTHS[step as usize]
}

type Callback = Box<dyn Fn() + Send + Sync>;

/// A registered observer of the fonts-changed notification, removed when dropped.
struct Observer {
    center: CFNotificationCenterRef,
    callback: *mut Callback,
}

// SAFETY: the observer registration is identified by an address CoreFoundation only
// compares, and the callback it points to is `Send + Sync`.
unsafe impl Send for Observer {}

impl Observer {
    fn new(on_change: Callback) -> Observer {
        let callback = Box::into_raw(Box::new(on_change));
        // SAFETY: the callback box outlives the registration, which `Drop` removes first.
        let center = unsafe {
            let center = CFNotificationCenterGetLocalCenter();
            let fonts_changed: CFNotificationCallback = fonts_changed;
            CFNotificationCenterAddObserver(
                center,
                callback.cast::<c_void>(),
                fonts_changed,
                kCTFontManagerRegisteredFontsChangedNotification,
                std::ptr::null(),
                CFNotificationSuspensionBehaviorDeliverImmediately,
            );
            center
        };
        Observer { center, callback }
    }
}

impl Drop for Observer {
    fn drop(&mut self) {
        // SAFETY: removing the registration before freeing the box it pointed to.
        unsafe {
            CFNotificationCenterRemoveObserver(
                self.center,
                self.callback.cast::<c_void>(),
                kCTFontManagerRegisteredFontsChangedNotification,
                std::ptr::null(),
            );
            drop(Box::from_raw(self.callback));
        }
    }
}

extern "C" fn fonts_changed(
    _center: CFNotificationCenterRef,
    observer: *mut c_void,
    _name: CFStringRef,
    _object: *const c_void,
    _user_info: core_foundation_sys::dictionary::CFDictionaryRef,
) {
    // SAFETY: `observer` is the callback box `Observer::new` registered, alive until the
    // registration is removed.
    let callback = unsafe { &*observer.cast::<Callback>() };
    callback();
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn coretext_weights_map_onto_css_weights() {
        assert_eq!(css_weight(0.0), 400);
        assert_eq!(css_weight(0.4), 700);
        assert_eq!(css_weight(-0.4), 300);
        assert_eq!(css_weight(0.23), 500);
        assert_eq!(css_weight(2.0), 1000);
    }

    #[test]
    fn coretext_widths_map_onto_css_percentages() {
        assert_eq!(css_width(0.0), 100.0);
        assert_eq!(css_width(-0.25), 87.5);
        assert_eq!(css_width(1.0), 200.0);
    }
}
