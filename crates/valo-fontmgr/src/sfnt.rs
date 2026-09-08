//! A font's tables as a platform serves them, and an sfnt assembled from them.

use skrifa::raw::FontRef;
use skrifa::raw::types::Tag;

/// The tables that hold glyph shapes and color glyphs. A platform keeps them: shapes are
/// rasterized on request, and a font file never leaves it.
const OUTLINE_TABLES: [&[u8; 4]; 14] = [
    b"glyf", b"loca", b"gvar", b"CFF ", b"CFF2", b"CBDT", b"CBLC", b"EBDT", b"EBLC", b"EBSC",
    b"sbix", b"SVG ", b"COLR", b"CPAL",
];

/// `served_tables` is face `index`'s tables in `bytes`, in the file's order, without the
/// outline and color-glyph tables. `None` when the file does not parse.
pub(crate) fn served_tables(bytes: &[u8], index: u32) -> Option<Vec<(Tag, &[u8])>> {
    let font = FontRef::from_index(bytes, index).ok()?;
    let mut tables = Vec::new();
    for record in font.table_directory().table_records() {
        let tag = record.tag();
        if OUTLINE_TABLES.contains(&&tag.to_be_bytes()) {
            continue;
        }
        let start = record.offset() as usize;
        let end = start.checked_add(record.length() as usize)?;
        tables.push((tag, bytes.get(start..end)?));
    }
    Some(tables)
}

/// `assemble` writes a single-face TrueType-flavoured sfnt holding `tables`, in the
/// order given, for a font parser to read. Table checksums are not computed, which no
/// parser verifies.
pub fn assemble(tables: &[(Tag, &[u8])]) -> Vec<u8> {
    const TRUETYPE: u32 = 0x0001_0000;
    let count = tables.len() as u16;
    let mut out = Vec::new();
    out.extend_from_slice(&TRUETYPE.to_be_bytes());
    out.extend_from_slice(&count.to_be_bytes());
    let (search_range, entry_selector, range_shift) = binary_search_fields(count);
    out.extend_from_slice(&search_range.to_be_bytes());
    out.extend_from_slice(&entry_selector.to_be_bytes());
    out.extend_from_slice(&range_shift.to_be_bytes());

    let mut offset = 12 + 16 * tables.len();
    for (tag, data) in tables {
        out.extend_from_slice(&tag.to_be_bytes());
        out.extend_from_slice(&0u32.to_be_bytes());
        out.extend_from_slice(&(offset as u32).to_be_bytes());
        out.extend_from_slice(&(data.len() as u32).to_be_bytes());
        offset += padded(data.len());
    }
    for (_, data) in tables {
        out.extend_from_slice(data);
        out.resize(out.len() + padded(data.len()) - data.len(), 0);
    }
    out
}

/// Table data starts on 4-byte boundaries.
fn padded(length: usize) -> usize {
    (length + 3) & !3
}

/// The header's binary-search hints, as the OpenType specification defines them.
fn binary_search_fields(count: u16) -> (u16, u16, u16) {
    let mut entry_selector = 0u16;
    while (2u16 << entry_selector) <= count {
        entry_selector += 1;
    }
    let search_range = (1u16 << entry_selector) * 16;
    (search_range, entry_selector, count * 16 - search_range)
}

#[cfg(test)]
mod tests {
    use super::*;
    use skrifa::MetadataProvider;

    #[test]
    fn binary_search_fields_follow_the_specification() {
        assert_eq!(binary_search_fields(12), (128, 3, 64));
        assert_eq!(binary_search_fields(16), (256, 4, 0));
    }

    #[test]
    fn served_tables_keep_shaping_and_lose_outlines() {
        let path = concat!(
            env!("CARGO_MANIFEST_DIR"),
            "/../../assets/fonts/fira_sans.ttf"
        );
        let bytes = std::fs::read(path).expect("fira_sans.ttf");
        let tables = served_tables(&bytes, 0).expect("a parsable font");
        let tags: Vec<[u8; 4]> = tables.iter().map(|(tag, _)| tag.to_be_bytes()).collect();
        assert!(tags.contains(b"cmap") && tags.contains(b"hmtx") && tags.contains(b"GSUB"));
        assert!(!tags.contains(b"glyf") && !tags.contains(b"loca"));

        let assembled = assemble(&tables);
        assert!(
            assembled.len() < bytes.len() / 2,
            "{} of {}",
            assembled.len(),
            bytes.len()
        );
        let font = FontRef::new(&assembled).expect("the assembled font parses");
        assert!(font.charmap().map('a').is_some());
        assert_eq!(font.table_directory().table_records().len(), tables.len());
    }
}
