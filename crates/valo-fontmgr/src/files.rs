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

/// `Files` reads each font file once and hands out its shared bytes.
#[cfg(not(target_arch = "wasm32"))]
#[derive(Default)]
pub(crate) struct Files {
    loaded: HashMap<PathBuf, FontData>,
}

#[cfg(not(target_arch = "wasm32"))]
impl Files {
    /// `read` returns the file's bytes, reading them on first use; `None` when the file
    /// cannot be read.
    pub(crate) fn read(&mut self, path: &Path) -> Option<FontData> {
        if let Some(data) = self.loaded.get(path) {
            return Some(Arc::clone(data));
        }
        let bytes = std::fs::read(path).ok()?;
        let data: FontData = Arc::new(bytes);
        self.loaded.insert(path.to_path_buf(), Arc::clone(&data));
        Some(data)
    }
}

/// `face_index` finds the face in `bytes` whose PostScript name is `postscript_name`,
/// which is how a platform API identifies one face of a collection file. `None` when no
/// face has that name; a single-face file answers 0 without a name check.
#[cfg(not(target_arch = "wasm32"))]
pub(crate) fn face_index(bytes: &[u8], postscript_name: &str) -> Option<u32> {
    match FileRef::new(bytes).ok()? {
        FileRef::Font(_) => Some(0),
        FileRef::Collection(collection) => (0..collection.len()).find(|&index| {
            collection
                .get(index)
                .ok()
                .and_then(|face| {
                    face.localized_strings(StringId::POSTSCRIPT_NAME)
                        .english_or_first()
                })
                .is_some_and(|name| name.to_string() == postscript_name)
        }),
    }
}

/// `covers` probes one face's character map without parsing the rest of it.
pub(crate) fn covers(bytes: &[u8], index: u32, character: char) -> bool {
    skrifa::FontRef::from_index(bytes, index)
        .map(|face| face.charmap().map(character).is_some())
        .unwrap_or(false)
}
