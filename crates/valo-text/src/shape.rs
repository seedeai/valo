use std::ops::Range;

use unicode_bidi::BidiInfo;
use valo_geometry::Color;

use crate::font::{FaceSet, FontCollection, FontDemand, FontId};
use crate::style::{
    Decoration, PlaceholderAlignment, PlaceholderStyle, Shadow, TextBaseline, TextStyle,
};
use crate::synthesis::FontSynthesis;

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
    /// The bold and italic the run fakes because its font lacks them.
    pub synthesis: FontSynthesis,
    pub color: Color,
    /// Line-height multiplier from the style (`None` = font metrics).
    pub height: Option<f32>,
    pub decoration: Option<Decoration>,
    pub shadows: Vec<Shadow>,
    pub glyphs: Vec<ShapedGlyph>,
    /// The placeholder this run stands for: its one glyph is the box's advance, and it has
    /// nothing to draw.
    pub placeholder: Option<ShapedPlaceholder>,
}

/// `ShapedPlaceholder` is a placeholder's box in place of glyphs: its index in the order the
/// placeholders were added, its style, and the font metrics of the text style it was added
/// with, which `Top`, `Bottom` and `Middle` measure against.
#[derive(Clone, Copy, Debug)]
pub struct ShapedPlaceholder {
    pub index: usize,
    pub style: PlaceholderStyle,
    pub font_ascent: f32,
    pub font_descent: f32,
}

impl ShapedPlaceholder {
    /// The box's (ascent, descent) on a line whose text descends `line_descent` below the
    /// baseline: Skia's `Run::updateMetrics`, with ascents measured upward.
    pub fn extent(&self, line_descent: f32) -> (f32, f32) {
        let PlaceholderStyle {
            height,
            baseline_offset: offset,
            ..
        } = self.style;
        let shift = match self.style.baseline {
            TextBaseline::Alphabetic => 0.0,
            TextBaseline::Ideographic => line_descent / 2.0,
        };
        match self.style.alignment {
            PlaceholderAlignment::Baseline => (offset - shift, shift + height - offset),
            PlaceholderAlignment::AboveBaseline => (height - shift, shift),
            PlaceholderAlignment::BelowBaseline => (-shift, shift + height),
            PlaceholderAlignment::Top => (self.font_ascent, height - self.font_ascent),
            PlaceholderAlignment::Bottom => (height - self.font_descent, self.font_descent),
            PlaceholderAlignment::Middle => {
                let middle = (self.font_ascent - self.font_descent) / 2.0;
                (height / 2.0 + middle, height / 2.0 - middle)
            }
        }
    }
}

/// Split the paragraph into uniform runs and shape each. `spans` cover the
/// text exactly, in order; `placeholders` are the byte offsets of the spans that
/// stand for placeholder boxes, in the order added.
pub fn shape_runs(
    collection: &mut FontCollection,
    text: &str,
    spans: &[(Range<usize>, TextStyle)],
    placeholders: &[(usize, PlaceholderStyle)],
    bidi: &BidiInfo,
    demand: &mut FontDemand,
) -> Vec<ShapedRun> {
    let mut runs = Vec::new();
    for (span, style) in spans {
        if let Some(index) = placeholders.iter().position(|(at, _)| *at == span.start) {
            let placeholder = placeholders[index].1;
            runs.push(placeholder_run(
                collection.faces(),
                span.clone(),
                style,
                index,
                placeholder,
            ));
            continue;
        }
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
    let attrs = style.font_attrs();
    let mut faces = collection.faces().style_faces(&style.families, attrs);
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
                let mut resolved = collection.faces().resolve_prepared(&faces, ch);
                let covered = matches!(resolved, Some((_, true)));
                if !covered && !ch.is_whitespace() {
                    // Uncovered ink pulls a face from the sources, which
                    // changes what the style resolves to; a char no source
                    // can render is skipped (its miss is recorded on the
                    // collection for the host's async loader).
                    if collection.require_codepoint(&style.families, ch, attrs) {
                        faces = collection.faces().style_faces(&style.families, attrs);
                        resolved = collection.faces().resolve_prepared(&faces, ch);
                    } else {
                        demand.add_codepoint(ch, attrs);
                    }
                }
                let Some((font, _covered)) = resolved else {
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
    let features = shaping_features(style);
    let mut glyphs: Vec<ShapedGlyph> =
        harf_shape(font, &text[segment.range.clone()], segment.level, &features)
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
        synthesis: FontSynthesis::needed(style.font_attrs(), font.attrs(), style.size),
        color: style.color,
        height: style.height,
        decoration: style.decoration,
        shadows: style.shadows.clone(),
        glyphs,
        placeholder: None,
    }
}

/// A placeholder's run: one cluster as wide as the box, with the metrics of the font its
/// text style resolves to. Without any faces the font is never read, so its id is a stand-in.
fn placeholder_run(
    faces: &FaceSet,
    range: Range<usize>,
    style: &TextStyle,
    index: usize,
    placeholder: PlaceholderStyle,
) -> ShapedRun {
    let (font, font_ascent, font_descent) = if faces.is_empty() {
        (FontId(0), 0.0, 0.0)
    } else {
        let id = faces.resolve(&style.families, style.font_attrs(), ' ');
        let face = faces.get(id);
        (id, face.ascent_px(style.size), face.descent_px(style.size))
    };
    ShapedRun {
        glyphs: vec![ShapedGlyph {
            id: 0,
            cluster: range.start,
            x_advance: placeholder.width,
            x_offset: 0.0,
            y_offset: 0.0,
        }],
        range,
        font,
        size: style.size,
        synthesis: FontSynthesis::NONE,
        color: style.color,
        height: None,
        decoration: None,
        shadows: Vec::new(),
        placeholder: Some(ShapedPlaceholder {
            index,
            style: placeholder,
            font_ascent,
            font_descent,
        }),
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
/// segmentation, LTR, clusters zeroed (it's placed as an opaque unit). It fakes
/// no styles: Skia shapes the ellipsis in a fresh font of the run's typeface.
pub(crate) fn shape_isolated(
    collection: &FaceSet,
    font_id: FontId,
    size: f32,
    color: Color,
    text: &str,
) -> ShapedRun {
    let font = collection.get(font_id);
    let scale = size / font.units_per_em();
    let glyphs = harf_shape(font, text, 0, &[])
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
        synthesis: FontSynthesis::NONE,
        color,
        height: None,
        decoration: None,
        shadows: Vec::new(),
        glyphs,
        placeholder: None,
    }
}

/// The style's OpenType feature overrides. Only DEVIATIONS from the shaper's
/// defaults appear: `kern` is on by default, so it is listed solely to turn
/// it off, and an untouched `VariantCaps::Normal` contributes nothing.
fn shaping_features(style: &TextStyle) -> Vec<harfrust::Feature> {
    let mut features = Vec::new();
    if !style.kerning {
        features.push(harfrust::Feature::new(harfrust::Tag::new(b"kern"), 0, ..));
    }
    for tag in style.variant_caps.feature_tags() {
        features.push(harfrust::Feature::new(harfrust::Tag::new(tag), 1, ..));
    }
    features
}

/// The harfrust boundary: text in, (glyph id, cluster byte, advance/offsets
/// in FONT UNITS) out. RTL levels shape right-to-left — glyphs come back in
/// visual order within the run. The expensive `ShaperData` is cached on the
/// font; only the cheap `FontRef` reconstructs per call.
fn harf_shape(
    font: &crate::font::Font,
    text: &str,
    level: u8,
    features: &[harfrust::Feature],
) -> Vec<(u32, usize, f32, f32, f32)> {
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
    let output = shaper.shape(buffer, features);
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
