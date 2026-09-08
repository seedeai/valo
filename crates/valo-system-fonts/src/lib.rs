//! The platform's installed fonts as valo's [`FontSource`].
//!
//! [`SystemFonts`] adapts a [`FontManager`], the platform's font lookup in Skia's shape,
//! to the two questions valo's font collection asks a source: the faces of a family, and
//! a face covering a character. The manager decides how the platform is asked (CoreText,
//! a directory scan, or a host across a boundary); this crate parses the answers into
//! [`Font`]s and picks the instance nearest the request, so it builds anywhere
//! `valo-text` does, wasm included.

pub use valo_fontmgr::{self as fontmgr, Watch};
use valo_fontmgr::{FontManager, Slant, Style, Typeface};
use valo_text::{FaceSet, Font, FontAttrs, FontDemand, FontSource};

/// `SystemFonts` is a [`FontSource`] over the platform's font manager.
///
/// Returned [`Font`] values share their file's bytes. For automatic resolution during
/// paragraph building, add it to a [`valo_text::FontCollection`]; [`satisfy`] is for a
/// host that resolves demands out of band.
///
/// [`satisfy`]: SystemFonts::satisfy
pub struct SystemFonts {
    manager: Box<dyn FontManager>,
    /// The languages fallback prefers, BCP 47, most preferred first.
    locales: Vec<String>,
}

impl SystemFonts {
    /// `load` uses this platform's own font manager. It may block while the platform
    /// indexes its fonts, so load once and keep it.
    #[cfg(not(target_arch = "wasm32"))]
    pub fn load() -> Self {
        Self::with_manager(valo_fontmgr::platform())
    }

    /// `with_manager` answers through `manager`, for a host that supplies its own.
    pub fn with_manager(manager: Box<dyn FontManager>) -> Self {
        SystemFonts {
            manager,
            locales: Vec::new(),
        }
    }

    /// `set_locales` tells character fallback which languages to prefer (BCP 47, most
    /// preferred first); none prefers whatever the platform does.
    pub fn set_locales(&mut self, locales: Vec<String>) {
        self.locales = locales;
    }

    /// `manager` is the platform lookup underneath, for questions valo does not ask.
    pub fn manager(&mut self) -> &mut dyn FontManager {
        &mut *self.manager
    }

    /// `face_count` is how many installed faces the platform reports.
    pub fn face_count(&self) -> usize {
        self.manager.face_count()
    }

    /// `system_family` is every weight of the platform's user-interface font for text at
    /// `size` points, one [`Font`] per weight the platform has; empty where the platform
    /// has no such notion. Registered under a name of the caller's, it stands in for
    /// "the system font".
    pub fn system_family(&mut self, size: f32) -> Vec<Font> {
        let mut fonts: Vec<Font> = Vec::new();
        for weight in (100..=900).step_by(100) {
            let attrs = FontAttrs {
                weight,
                ..FontAttrs::default()
            };
            let Some(typeface) = self.manager.system_font(size, style_of(attrs)) else {
                continue;
            };
            let Some(font) = nearest_instance(fonts_of(typeface), attrs) else {
                continue;
            };
            if !fonts.iter().any(|known| known.attrs() == font.attrs()) {
                fonts.push(font);
            }
        }
        fonts
    }

    /// `satisfy` returns a face-set clone extended to answer a font demand, or `None`
    /// when no installed font answers any of it.
    pub fn satisfy(&mut self, faces: &FaceSet, demand: &FontDemand) -> Option<FaceSet> {
        faces.grown_by(self, demand)
    }
}

impl FontSource for SystemFonts {
    fn family(&mut self, name: &str) -> Vec<Font> {
        self.manager
            .family(name)
            .into_iter()
            .flat_map(fonts_of)
            .collect()
    }

    fn face_for_codepoint(&mut self, codepoint: char, attrs: FontAttrs) -> Option<Font> {
        let locales: Vec<&str> = self.locales.iter().map(String::as_str).collect();
        let typeface = self
            .manager
            .match_character(None, style_of(attrs), &locales, codepoint)?;
        nearest_instance(fonts_of(typeface), attrs).filter(|font| font.covers(codepoint))
    }
}

/// Every registrable instance of a typeface: one for a static face, one per named
/// instance of a variable one.
fn fonts_of(typeface: Typeface) -> Vec<Font> {
    Font::instances_from_data(typeface.shared_data(), typeface.index())
}

/// A variable face answers with its instance nearest the request (a bold span's
/// fallback arrives bold); static fonts pass through unchanged.
fn nearest_instance(instances: Vec<Font>, attrs: FontAttrs) -> Option<Font> {
    instances.into_iter().min_by_key(|face| {
        (
            face.attrs().italic != attrs.italic,
            face.attrs().weight.abs_diff(attrs.weight),
        )
    })
}

fn style_of(attrs: FontAttrs) -> Style {
    Style {
        weight: attrs.weight,
        width: attrs.stretch,
        slant: if attrs.italic {
            Slant::Italic
        } else {
            Slant::Upright
        },
    }
}
