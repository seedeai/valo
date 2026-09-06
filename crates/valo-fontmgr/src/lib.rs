//! Installed fonts, found the way Skia's `SkFontMgr` finds them.
//!
//! A [`FontManager`] answers the questions a text stack asks the platform: which faces a
//! family has, which face covers a character in a language, and which face is the
//! platform's own user-interface font. Each platform answers through its font API
//! (CoreText on Apple systems) or, where there is no API yet, a scan of its font
//! directories; every answer is a [`Typeface`], the font file's bytes and the face's index
//! in them, which any font parser reads. The crate carries no text stack of its own, so a
//! host that only forwards fonts depends on it alone; `valo-system-fonts` turns a manager
//! into valo's `FontSource`. On wasm the trait and types build with no backend, for a
//! program whose host answers across a boundary.

use std::any::Any;
use std::sync::Arc;

#[cfg(any(target_os = "macos", target_os = "ios"))]
mod coretext;
mod files;
#[cfg(not(target_arch = "wasm32"))]
mod scan;

#[cfg(any(target_os = "macos", target_os = "ios"))]
pub use coretext::CoreText;
#[cfg(not(target_arch = "wasm32"))]
pub use scan::Scan;

/// `FontData` is a font file's bytes, shared by every face read from them.
pub type FontData = Arc<dyn AsRef<[u8]> + Send + Sync>;

/// `Slant` is a face's posture: upright, a designed italic, or a slanted upright.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum Slant {
    #[default]
    Upright,
    Italic,
    Oblique,
}

/// `Style` is a face's position within its family, in CSS terms (Skia's `SkFontStyle`).
///
/// Matching compares styles by width first, then slant, then weight, the CSS order.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Style {
    /// `weight` is the CSS font weight, conventionally 100 to 900.
    pub weight: u16,
    /// `width` is the CSS font-width percentage; 100 is normal.
    pub width: f32,
    pub slant: Slant,
}

/// `NORMAL_WIDTH` is the CSS normal font-width percentage.
pub const NORMAL_WIDTH: f32 = 100.0;

impl Default for Style {
    fn default() -> Self {
        Style {
            weight: 400,
            width: NORMAL_WIDTH,
            slant: Slant::Upright,
        }
    }
}

impl Style {
    /// `distance` orders candidates for a requested style: width, then slant, then weight.
    fn distance(self, requested: Style) -> (u32, u8, u16) {
        let width = ((self.width - requested.width).abs() * 10.0) as u32;
        let slant = match (requested.slant, self.slant) {
            (a, b) if a == b => 0,
            (Slant::Italic, Slant::Oblique) | (Slant::Oblique, Slant::Italic) => 1,
            _ => 2,
        };
        (width, slant, self.weight.abs_diff(requested.weight))
    }
}

/// `Typeface` is one face the platform matched: a font file, the face's index in it, and
/// how the platform names and styles that face.
///
/// The bytes are shared, so many typefaces from one collection file carry one copy. A
/// variable font is one typeface; its instances are the parser's to enumerate.
#[derive(Clone)]
pub struct Typeface {
    data: FontData,
    index: u32,
    family: String,
    style: Style,
}

impl Typeface {
    pub fn new(data: FontData, index: u32, family: impl Into<String>, style: Style) -> Typeface {
        Typeface {
            data,
            index,
            family: family.into(),
            style,
        }
    }

    /// `data` is the whole font file.
    pub fn data(&self) -> &[u8] {
        (*self.data).as_ref()
    }

    /// `shared_data` is the file's bytes as a font parser shares them.
    pub fn shared_data(&self) -> FontData {
        Arc::clone(&self.data)
    }

    /// `index` is the face's position in a collection file; 0 in a single-face file.
    pub fn index(&self) -> u32 {
        self.index
    }

    /// `family` is the name the platform files the face under.
    pub fn family(&self) -> &str {
        &self.family
    }

    pub fn style(&self) -> Style {
        self.style
    }

    /// `covers` reports whether the face has a glyph for `character`.
    pub fn covers(&self, character: char) -> bool {
        files::covers(self.data(), self.index, character)
    }
}

impl std::fmt::Debug for Typeface {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Typeface")
            .field("family", &self.family)
            .field("index", &self.index)
            .field("style", &self.style)
            .finish_non_exhaustive()
    }
}

/// `FontManager` is the platform's font lookup, in the shape of Skia's `SkFontMgr`.
///
/// Answers are typefaces the caller parses; nothing is shaped or rasterised here. A
/// manager may read font files lazily and cache them, hence `&mut self`.
pub trait FontManager: Send {
    /// `family` returns every face the platform has under `family`; empty for a family
    /// it does not know. Names compare the way the platform compares them.
    fn family(&mut self, family: &str) -> Vec<Typeface>;

    /// `match_family_style` returns the family's face nearest `style`, Skia's
    /// `matchFamilyStyle`.
    fn match_family_style(&mut self, family: &str, style: Style) -> Option<Typeface> {
        nearest(self.family(family), style)
    }

    /// `match_character` returns a face that covers `character`, Skia's
    /// `matchFamilyStyleCharacter`: preferring the languages in `locales` (BCP 47, most
    /// preferred first), then `family`, then `style`. `None` when nothing installed
    /// covers it.
    fn match_character(
        &mut self,
        family: Option<&str>,
        style: Style,
        locales: &[&str],
        character: char,
    ) -> Option<Typeface>;

    /// `system_font` returns the platform's own user-interface face for text at `size`
    /// (in points) nearest `style`: what a program means by "the system font". `None`
    /// where the platform has no such notion.
    fn system_font(&mut self, size: f32, style: Style) -> Option<Typeface>;

    /// `families` lists the installed family names.
    fn families(&self) -> Vec<String>;

    /// `face_count` is how many installed faces the platform reports; 0 means the
    /// manager found nothing and every answer will be empty.
    fn face_count(&self) -> usize;

    /// `watch` calls `on_change` whenever the installed fonts change, from whichever
    /// thread the platform reports on, for as long as the returned [`Watch`] lives.
    /// `None` where the platform cannot say.
    fn watch(&mut self, on_change: Box<dyn Fn() + Send + Sync>) -> Option<Watch>;
}

/// `Watch` is a subscription to the installed fonts changing; drop it to stop.
pub struct Watch {
    _subscription: Box<dyn Any + Send>,
}

impl Watch {
    pub fn new(subscription: impl Any + Send) -> Watch {
        Watch {
            _subscription: Box::new(subscription),
        }
    }
}

/// `nearest` picks the face closest to `style`: width first, then slant, then weight,
/// the CSS order.
pub fn nearest(faces: Vec<Typeface>, style: Style) -> Option<Typeface> {
    faces
        .into_iter()
        .min_by_key(|face| face.style().distance(style))
}

/// `platform` is this platform's font manager: CoreText on macOS and iOS, a directory
/// scan elsewhere.
#[cfg(not(target_arch = "wasm32"))]
pub fn platform() -> Box<dyn FontManager> {
    #[cfg(any(target_os = "macos", target_os = "ios"))]
    {
        Box::new(CoreText::new())
    }
    #[cfg(not(any(target_os = "macos", target_os = "ios")))]
    {
        Box::new(Scan::load())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn face(family: &str, weight: u16, width: f32, slant: Slant) -> Typeface {
        Typeface::new(
            Arc::new(Vec::new()),
            0,
            family,
            Style {
                weight,
                width,
                slant,
            },
        )
    }

    #[test]
    fn nearest_prefers_width_then_slant_then_weight() {
        let faces = vec![
            face("a", 400, 100.0, Slant::Italic),
            face("b", 700, 100.0, Slant::Upright),
            face("c", 400, 75.0, Slant::Upright),
        ];
        let regular = Style::default();
        assert_eq!(nearest(faces.clone(), regular).unwrap().family(), "b");
        let italic = Style {
            slant: Slant::Italic,
            ..regular
        };
        assert_eq!(nearest(faces.clone(), italic).unwrap().family(), "a");
        let condensed = Style {
            width: 75.0,
            ..regular
        };
        assert_eq!(nearest(faces, condensed).unwrap().family(), "c");
    }

    #[test]
    fn oblique_stands_in_for_italic_before_upright() {
        let faces = vec![
            face("upright", 400, 100.0, Slant::Upright),
            face("oblique", 400, 100.0, Slant::Oblique),
        ];
        let italic = Style {
            slant: Slant::Italic,
            ..Style::default()
        };
        assert_eq!(nearest(faces, italic).unwrap().family(), "oblique");
    }
}
