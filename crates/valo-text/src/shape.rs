use std::ops::Range;

use unicode_bidi::BidiInfo;
use valo_geometry::Color;

use crate::font::{FaceSet, FontAttrs, FontCollection, FontDemand, FontId};
use crate::style::{Decoration, Shadow, TextStyle};

/// One shaped glyph, unpositioned: advances/offsets in px, `cluster` a byte
/// index into the paragraph text (the wrap slicer's key).
#[derive(Clone, Copy, Debug)]
pub struct ShapedGlyph {
    pub id: u32,
    pub cluster: usize,
    pub x_advance: f32,
    pub x_offset: f32,
    pub y_offset: f32,
}

/// A maximal stretch of text sharing style, resolved font, and bidi level —
/// shaped as one harfrust call on the "endless line" (width-free; wrapping
/// slices clusters later).
#[derive(Clone, Debug)]
pub struct ShapedRun {
    pub range: Range<usize>,
    pub font: FontId,
    pub size: f32,
    pub color: Color,
    /// Line-height multiplier from the style (`None` = font metrics).
    pub height: Option<f32>,
    pub decoration: Option<Decoration>,
    pub shadows: Vec<Shadow>,
    pub glyphs: Vec<ShapedGlyph>,
}

/// Split the paragraph into uniform runs and shape each. `spans` cover the
/// text exactly, in order.
pub fn shape_runs(
    collection: &mut FontCollection,
    text: &str,
    spans: &[(Range<usize>, TextStyle)],
    bidi: &BidiInfo,
    demand: &mut FontDemand,
) -> Vec<ShapedRun> {
    let mut runs = Vec::new();
    for (span, style) in spans {
        // The by-name miss point: the collection consults its own sources
        // (Skia `findTypefaces` walking its managers).
        for name in &style.families {
            if !collection.require_family(name) {
                demand.add_family(name);
            }
        }
        for segment in segment_span(collection, text, span.clone(), style, bidi, demand) {
            runs.push(shape_segment(
                collection.faces(),
                text,
                segment,
                style,
                bidi,
            ));
        }
    }
    runs
}

struct Segment {
    range: Range<usize>,
    font: FontId,
    level: u8,
}

/// Cut one styled span wherever the resolved font or the bidi level changes.
/// Whitespace sticks to the run it's in when covered (spaces
/// shouldn't split otherwise-uniform runs). Newlines separate silently.
fn segment_span(
    collection: &mut FontCollection,
    text: &str,
    span: Range<usize>,
    style: &TextStyle,
    bidi: &BidiInfo,
    demand: &mut FontDemand,
) -> Vec<Segment> {
    let mut segments: Vec<Segment> = Vec::new();
    for (at, ch) in text[span.clone()].char_indices() {
        let at = span.start + at;
        if ch == '\n' {
            continue;
        }
        let level = bidi.levels[at].number();
        let sticky = ch.is_whitespace();
        let font = match (&segments.last(), sticky) {
            (Some(last), true)
                if last.range.end == at
                    && last.level == level
                    && collection.faces().get(last.font).covers(ch) =>
            {
                last.font
            }
            _ => {
                let attrs = FontAttrs {
                    weight: style.weight,
                    italic: style.italic,
                };
                // Uncovered ink pulls a face from the sources; a char no
                // source can render is skipped (its miss is recorded on
                // the collection for the host's async loader).
                if !ch.is_whitespace() && !collection.require_codepoint(ch, attrs) {
                    demand.add_codepoint(ch, attrs);
                }
                let Some((font, _covered)) =
                    collection
                        .faces()
                        .resolve_covered_opt(&style.families, attrs, ch)
                else {
                    continue;
                };
                font
            }
        };
        match segments.last_mut() {
            Some(last) if last.range.end == at && last.font == font && last.level == level => {
                last.range.end = at + ch.len_utf8();
            }
            _ => segments.push(Segment {
                range: at..at + ch.len_utf8(),
                font,
                level,
            }),
        }
    }
    segments
}

fn shape_segment(
    collection: &FaceSet,
    text: &str,
    segment: Segment,
    style: &TextStyle,
    bidi: &BidiInfo,
) -> ShapedRun {
    let _ = bidi;
    let font = collection.get(segment.font);
    let scale = style.size / font.units_per_em();
    let mut glyphs: Vec<ShapedGlyph> =
        harf_shape(font, &text[segment.range.clone()], segment.level)
            .into_iter()
            .map(|(id, cluster, adv, dx, dy)| ShapedGlyph {
                id,
                cluster: segment.range.start + cluster,
                x_advance: adv * scale,
                x_offset: dx * scale,
                y_offset: dy * scale,
            })
            .collect();
    apply_spacing(&mut glyphs, text, style);
    ShapedRun {
        range: segment.range,
        font: segment.font,
        size: style.size,
        color: style.color,
        height: style.height,
        decoration: style.decoration,
        shadows: style.shadows.clone(),
        glyphs,
    }
}

/// Letter spacing lands after every cluster, word spacing additionally
/// after U+0020 clusters — applied to advances at shape time so wrapping,
/// justify, and intrinsics all measure the same text (skparagraph's rule).
fn apply_spacing(glyphs: &mut [ShapedGlyph], text: &str, style: &TextStyle) {
    if style.letter_spacing == 0.0 && style.word_spacing == 0.0 {
        return;
    }
    for i in 0..glyphs.len() {
        let cluster_ends = glyphs
            .get(i + 1)
            .is_none_or(|next| next.cluster != glyphs[i].cluster);
        if !cluster_ends {
            continue;
        }
        glyphs[i].x_advance += style.letter_spacing;
        if text.as_bytes().get(glyphs[i].cluster) == Some(&b' ') {
            glyphs[i].x_advance += style.word_spacing;
        }
    }
}

/// Shape a short standalone string (the ellipsis) in one font/size — no
/// segmentation, LTR, clusters zeroed (it's placed as an opaque unit).
pub(crate) fn shape_isolated(
    collection: &FaceSet,
    font_id: FontId,
    size: f32,
    color: Color,
    text: &str,
) -> ShapedRun {
    let font = collection.get(font_id);
    let scale = size / font.units_per_em();
    let glyphs = harf_shape(font, text, 0)
        .into_iter()
        .map(|(id, _, adv, dx, dy)| ShapedGlyph {
            id,
            cluster: 0,
            x_advance: adv * scale,
            x_offset: dx * scale,
            y_offset: dy * scale,
        })
        .collect();
    ShapedRun {
        range: 0..0,
        font: font_id,
        size,
        color,
        height: None,
        decoration: None,
        shadows: Vec::new(),
        glyphs,
    }
}

/// The harfrust boundary: text in, (glyph id, cluster byte, advance/offsets
/// in FONT UNITS) out. RTL levels shape right-to-left — glyphs come back in
/// visual order within the run. The expensive `ShaperData` is cached on the
/// font; only the cheap `FontRef` reconstructs per call.
fn harf_shape(font: &crate::font::Font, text: &str, level: u8) -> Vec<(u32, usize, f32, f32, f32)> {
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
    buffer.set_direction(if level % 2 == 1 {
        harfrust::Direction::RightToLeft
    } else {
        harfrust::Direction::LeftToRight
    });
    buffer.guess_segment_properties();
    let output = shaper.shape(buffer, &[]);
    output
        .glyph_infos()
        .iter()
        .zip(output.glyph_positions())
        .map(|(info, pos)| {
            (
                info.glyph_id,
                info.cluster as usize,
                pos.x_advance as f32,
                pos.x_offset as f32,
                pos.y_offset as f32,
            )
        })
        .collect()
}
