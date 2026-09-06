//! Font files as backends read them: once each, shared among their faces, and probed
//! with skrifa for what a platform API does not say.

#[cfg(not(target_arch = "wasm32"))]
use std::collections::HashMap;
#[cfg(not(target_arch = "wasm32"))]
use std::path::{Path, PathBuf};
#[cfg(not(target_arch = "wasm32"))]
use std::sync::Arc;

#[cfg(not(target_arch = "wasm32"))]
use skrifa::raw::FileRef;
#[cfg(not(target_arch = "wasm32"))]
use skrifa::string::StringId;
use skrifa::MetadataProvider;

#[cfg(not(target_arch = "wasm32"))]
use crate::FontData;

/// `Files` maps each font file once and hands out its shared bytes.
///
/// Files are memory-mapped, not read: a platform's collection files run to tens of
/// megabytes, and a face touches a few tables of one. The mapping assumes the file does
/// not change while mapped, the assumption every mmap-based font stack shares; a font
/// uninstalled mid-run degrades glyphs, it does not race memory (the map holds the old
/// pages).
#[cfg(not(target_arch = "wasm32"))]
#[derive(Default)]
pub(crate) struct Files {
    loaded: HashMap<PathBuf, FontData>,
}

#[cfg(not(target_arch = "wasm32"))]
impl Files {
    /// `read` returns the file's bytes, mapping them on first use; `None` when the file
    /// cannot be opened.
    pub(crate) fn read(&mut self, path: &Path) -> Option<FontData> {
        if let Some(data) = self.loaded.get(path) {
            return Some(Arc::clone(data));
        }
        let file = std::fs::File::open(path).ok()?;
        // SAFETY: see the type's note; installed font files are treated as immutable.
        let map = unsafe { memmap2::Mmap::map(&file) }.ok()?;
        let data: FontData = Arc::new(map);
        self.loaded.insert(path.to_path_buf(), Arc::clone(&data));
        Some(data)
    }
}

/// `face_index` finds the face in `bytes` that a platform API names `postscript_name`.
/// A single-face file answers 0 without a name check. In a collection, the face whose
/// PostScript name matches wins; failing that, the face whose name matches up to its
/// last `-`, because a variable face's file carries its default instance's name while
/// the platform names the instance it matched (`.PingFangUITextSC-Default` for
/// `.PingFangUITextSC-Regular`). `None` when no face is named like that.
#[cfg(not(target_arch = "wasm32"))]
pub(crate) fn face_index(bytes: &[u8], postscript_name: &str) -> Option<u32> {
    let collection = match FileRef::new(bytes).ok()? {
        FileRef::Font(_) => return Some(0),
        FileRef::Collection(collection) => collection,
    };
    let names: Vec<Option<String>> = (0..collection.len())
        .map(|index| {
            collection
                .get(index)
                .ok()
                .and_then(|face| {
                    face.localized_strings(StringId::POSTSCRIPT_NAME)
                        .english_or_first()
                })
                .map(|name| name.to_string())
        })
        .collect();
    let exact = names
        .iter()
        .position(|name| name.as_deref() == Some(postscript_name));
    if let Some(index) = exact {
        return Some(index as u32);
    }
    let stem = family_stem(postscript_name);
    names
        .iter()
        .position(|name| name.as_deref().map(family_stem) == Some(stem))
        .map(|index| index as u32)
}

/// A PostScript name without its instance suffix: `PingFangSC-Regular` is `PingFangSC`.
#[cfg(not(target_arch = "wasm32"))]
fn family_stem(postscript_name: &str) -> &str {
    postscript_name
        .rsplit_once('-')
        .map_or(postscript_name, |(stem, _)| stem)
}

/// `covers` probes one face's character map without parsing the rest of it.
pub(crate) fn covers(bytes: &[u8], index: u32, character: char) -> bool {
    skrifa::FontRef::from_index(bytes, index)
        .map(|face| face.charmap().map(character).is_some())
        .unwrap_or(false)
}
