//! A directory scan as the font manager, for platforms without a font API binding yet.
//!
//! fontdb walks the platform's font directories, so this backend knows families, styles
//! and coverage but nothing the operating system decides: no language preference in
//! fallback, no user-interface font, no change notification.

use crate::files;
use crate::{FontData, FontManager, Slant, Style, Typeface, Watch, NORMAL_WIDTH};

/// `Scan` is an index of the fonts found under the platform's font directories.
///
/// Creating it walks those directories and may block, so create it once.
pub struct Scan {
    database: fontdb::Database,
}

impl Scan {
    /// `load` walks the platform's font directories now.
    pub fn load() -> Scan {
        let mut database = fontdb::Database::new();
        database.load_system_fonts();
        Scan { database }
    }

    fn typeface(&mut self, id: fontdb::ID) -> Option<Typeface> {
        let (family, style) = {
            let face = self.database.face(id)?;
            (face.families.first()?.0.clone(), style_of(face))
        };
        // SAFETY (fontdb's mmap contract): the font file must not change while mapped.
        // Installed fonts are effectively immutable while in use, the assumption every
        // mmap-based font stack shares; a font uninstalled mid-run degrades glyphs, it
        // does not race memory we hand out (the map holds the old pages).
        let (data, index) = unsafe { self.database.make_shared_face_data(id)? };
        let data: FontData = data;
        Some(Typeface::new(data, index, family, style))
    }

    /// Every face, nearest to `style` first, those of `family` before the rest.
    fn candidates(&self, family: Option<&str>, style: Style) -> Vec<fontdb::ID> {
        let mut keyed: Vec<(Rank, fontdb::ID)> = self
            .database
            .faces()
            .map(|face| {
                let in_family = family.is_some_and(|name| answers_to(face, name));
                ((!in_family, style_of(face).distance(style)), face.id)
            })
            .collect();
        keyed.sort_by(|a, b| a.0.partial_cmp(&b.0).unwrap_or(std::cmp::Ordering::Equal));
        keyed.into_iter().map(|(_, id)| id).collect()
    }
}

impl FontManager for Scan {
    fn family(&mut self, family: &str) -> Vec<Typeface> {
        let ids: Vec<fontdb::ID> = self
            .database
            .faces()
            .filter(|face| answers_to(face, family))
            .map(|face| face.id)
            .collect();
        ids.into_iter().filter_map(|id| self.typeface(id)).collect()
    }

    fn match_character(
        &mut self,
        family: Option<&str>,
        style: Style,
        _locales: &[&str],
        character: char,
    ) -> Option<Typeface> {
        for id in self.candidates(family, style) {
            let Some(typeface) = self.typeface(id) else {
                continue;
            };
            if files::covers(typeface.data(), typeface.index(), character) {
                return Some(typeface);
            }
        }
        None
    }

    fn system_font(&mut self, _size: f32, _style: Style) -> Option<Typeface> {
        None
    }

    fn families(&self) -> Vec<String> {
        let mut names: Vec<String> = self
            .database
            .faces()
            .filter_map(|face| face.families.first().map(|(name, _)| name.clone()))
            .collect();
        names.sort();
        names.dedup();
        names
    }

    fn face_count(&self) -> usize {
        self.database.len()
    }

    fn watch(&mut self, _on_change: Box<dyn Fn() + Send + Sync>) -> Option<Watch> {
        None
    }
}

/// A face's place in a search: outside the requested family first, then by style.
type Rank = (bool, (u32, u8, u16));

fn answers_to(face: &fontdb::FaceInfo, name: &str) -> bool {
    face.families
        .iter()
        .any(|(family, _)| family.eq_ignore_ascii_case(name))
}

fn style_of(face: &fontdb::FaceInfo) -> Style {
    Style {
        weight: face.weight.0,
        width: width_percent(face.stretch),
        slant: match face.style {
            fontdb::Style::Normal => Slant::Upright,
            fontdb::Style::Italic => Slant::Italic,
            fontdb::Style::Oblique => Slant::Oblique,
        },
    }
}

/// The CSS font-width percentage of a `usWidthClass` value.
fn width_percent(stretch: fontdb::Stretch) -> f32 {
    match stretch {
        fontdb::Stretch::UltraCondensed => 50.0,
        fontdb::Stretch::ExtraCondensed => 62.5,
        fontdb::Stretch::Condensed => 75.0,
        fontdb::Stretch::SemiCondensed => 87.5,
        fontdb::Stretch::Normal => NORMAL_WIDTH,
        fontdb::Stretch::SemiExpanded => 112.5,
        fontdb::Stretch::Expanded => 125.0,
        fontdb::Stretch::ExtraExpanded => 150.0,
        fontdb::Stretch::UltraExpanded => 200.0,
    }
}
