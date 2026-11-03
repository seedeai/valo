//! The shaper turns a string set in one font, at one size and in one
//! direction, into glyphs: HarfBuzz's role, played by harfrust, as Skia's
//! `SkShaper` plays it beneath its paragraph. Cutting styled text into such
//! strings, by style, resolved font and bidi level, is the paragraph's work
//! (Skia's `OneLineShaper`), in the crate above this one.

use crate::font::Font;

/// `FontFeature` is one OpenType feature setting handed to the shaper.
///
/// A `value` of 0 turns the feature off and 1 turns it on; features with
/// alternates take higher values to pick one. Features left out keep the
/// shaper's defaults, which already include kerning.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct FontFeature {
    /// `tag` is the feature's four-letter OpenType tag, such as `kern`.
    pub tag: [u8; 4],
    /// `value` is the feature's setting.
    pub value: u32,
}

/// `ShapedGlyph` is one glyph the shaper produced, before line layout places it.
#[derive(Clone, Copy, Debug)]
pub struct ShapedGlyph {
    /// `id` is the font-specific glyph identifier.
    pub id: u32,
    /// `cluster` is the UTF-8 byte offset of the text the glyph draws, counted
    /// from the start of the shaped string.
    pub cluster: usize,
    /// `x_advance` is how far the pen moves after the glyph, in logical pixels.
    pub x_advance: f32,
    /// `x_offset` moves the glyph right of the pen, in logical pixels.
    pub x_offset: f32,
    /// `y_offset` moves the glyph up from the baseline, in logical pixels.
    pub y_offset: f32,
}

/// `shape` turns `text` into glyphs of `font` at `size` logical pixels.
///
/// Glyphs come back in visual order, so a right-to-left string's first glyph
/// is its leftmost. `features` change the font's defaults; an empty list keeps
/// them. A font harfrust cannot read shapes to no glyphs. The expensive
/// shaping tables are cached on the font, so repeated calls rebuild only a
/// cheap reference to its bytes.
pub fn shape(
    font: &Font,
    size: f32,
    text: &str,
    right_to_left: bool,
    features: &[FontFeature],
) -> Vec<ShapedGlyph> {
    let Ok(font_ref) = harfrust::FontRef::from_index(font.data(), font.face_index()) else {
        return Vec::new();
    };
    let shaper = font
        .shaper_data()
        .shaper(&font_ref)
        .instance(font.shaper_instance())
        .build();
    let mut buffer = harfrust::UnicodeBuffer::new();
    buffer.push_str(text);
    buffer.set_direction(if right_to_left {
        harfrust::Direction::RightToLeft
    } else {
        harfrust::Direction::LeftToRight
    });
    buffer.guess_segment_properties();
    let features: Vec<harfrust::Feature> = features
        .iter()
        .map(|feature| harfrust::Feature::new(harfrust::Tag::new(&feature.tag), feature.value, ..))
        .collect();
    let output = shaper.shape(buffer, &features);
    let scale = size / font.units_per_em();
    output
        .glyph_infos()
        .iter()
        .zip(output.glyph_positions())
        .map(|(info, position)| ShapedGlyph {
            id: info.glyph_id,
            cluster: info.cluster as usize,
            x_advance: position.x_advance as f32 * scale,
            x_offset: position.x_offset as f32 * scale,
            y_offset: position.y_offset as f32 * scale,
        })
        .collect()
}
