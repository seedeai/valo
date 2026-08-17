//! Native system-font discovery for Valo.
//!
//! [`SystemFonts`] implements [`FontSource`] by scanning fonts installed on the
//! operating system. Keeping discovery in this separate crate prevents
//! `valo-text` and WebAssembly builds from acquiring platform filesystem code.

use valo_text::{FaceSet, Font, FontAttrs, FontDemand, FontSource};

/// `SystemFonts` is a reusable index of fonts installed on the operating system.
///
/// Creating it scans platform font directories and may block, so load it once
/// and retain it as a [`FontSource`]. Returned [`Font`] values retain shared
/// mappings of their font data.
pub struct SystemFonts {
    database: fontdb::Database,
}

impl SystemFonts {
    /// `load` synchronously scans platform font directories.
    pub fn load() -> Self {
        let mut database = fontdb::Database::new();
        database.load_system_fonts();
        Self { database }
    }

    /// `face_count` returns the number of installed faces discovered by the scan.
    pub fn face_count(&self) -> usize {
        self.database.len()
    }

    /// `satisfy` returns a face-set clone extended to answer a font demand.
    ///
    /// It returns `None` when no matching system font is found. For automatic
    /// resolution during paragraph building, add `SystemFonts` directly to a
    /// [`valo_text::FontCollection`] instead.
    pub fn satisfy(&mut self, faces: &FaceSet, demand: &FontDemand) -> Option<FaceSet> {
        faces.grown_by(self, demand)
    }

    /// All face identifiers, closest to `attrs` first (valo's CSS-style
    /// order: matching style, then smallest weight distance).
    fn face_identifiers_nearest(&self, attrs: FontAttrs) -> Vec<fontdb::ID> {
        let mut keyed: Vec<(bool, u16, fontdb::ID)> = self
            .database
            .faces()
            .map(|face| {
                let italic = face.style != fontdb::Style::Normal;
                (
                    italic != attrs.italic,
                    face.weight.0.abs_diff(attrs.weight),
                    face.id,
                )
            })
            .collect();
        keyed
            .sort_by_key(|&(style_mismatch, weight_distance, _)| (style_mismatch, weight_distance));
        keyed
            .into_iter()
            .map(|(_, _, identifier)| identifier)
            .collect()
    }

    fn shared_face_data(&mut self, identifier: fontdb::ID) -> Option<(valo_text::FontData, u32)> {
        // SAFETY (fontdb's mmap contract): the font file must not change
        // while mapped. Installed fonts are effectively immutable while in
        // use — the assumption every mmap-based font stack shares; a font
        // uninstalled mid-run degrades glyphs, it does not race memory we
        // hand out (the map holds the old pages).
        unsafe { self.database.make_shared_face_data(identifier) }
    }
}

impl FontSource for SystemFonts {
    fn family(&mut self, name: &str) -> Vec<Font> {
        let identifiers: Vec<fontdb::ID> = self
            .database
            .faces()
            .filter(|face| face_answers_to(face, name))
            .map(|face| face.id)
            .collect();
        identifiers
            .into_iter()
            .filter_map(|identifier| self.shared_face_data(identifier))
            .flat_map(|(data, face_index)| Font::instances_from_data(data, face_index))
            .collect()
    }

    fn face_for_codepoint(&mut self, codepoint: char, attrs: FontAttrs) -> Option<Font> {
        for identifier in self.face_identifiers_nearest(attrs) {
            let Some((data, face_index)) = self.shared_face_data(identifier) else {
                continue;
            };
            if face_covers((*data).as_ref(), face_index, codepoint) {
                return nearest_instance(Font::instances_from_data(data, face_index), attrs);
            }
        }
        None
    }
}

/// A variable face answers with its instance nearest the request (a bold
/// span's fallback arrives bold); static fonts pass through unchanged.
fn nearest_instance(instances: Vec<Font>, attrs: FontAttrs) -> Option<Font> {
    instances.into_iter().min_by_key(|face| {
        (
            face.attrs().italic != attrs.italic,
            face.attrs().weight.abs_diff(attrs.weight),
        )
    })
}

fn face_answers_to(face: &fontdb::FaceInfo, name: &str) -> bool {
    face.families
        .iter()
        .any(|(family, _)| family.eq_ignore_ascii_case(name))
}

/// Probe one face's cmap without building a full [`Font`] — candidates are
/// rejected wholesale during the coverage scan and must stay cheap.
fn face_covers(bytes: &[u8], face_index: u32, codepoint: char) -> bool {
    use skrifa::MetadataProvider;
    skrifa::FontRef::from_index(bytes, face_index)
        .map(|face| face.charmap().map(codepoint).is_some())
        .unwrap_or(false)
}
