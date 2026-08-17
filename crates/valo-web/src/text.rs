use valo::{
    Color, FontCollection, Paragraph, ParagraphBuilder, ParagraphStyle, TextAlign, TextDirection,
    TextStyle, VariantCaps,
};
use wasm_bindgen::prelude::*;

/// `WebFontCollection` holds the font files a paragraph can select from.
///
/// Register bytes before building text. There is no operating-system font
/// discovery on wasm; the host supplies every face. A built paragraph clones
/// the faces it used and remains independent of later registration.
#[wasm_bindgen(js_name = FontCollection)]
pub struct WebFontCollection {
    pub(crate) inner: FontCollection,
}

#[wasm_bindgen(js_class = FontCollection)]
impl WebFontCollection {
    /// `new` creates an empty collection with no fonts.
    #[wasm_bindgen(constructor)]
    pub fn new() -> WebFontCollection {
        WebFontCollection {
            inner: FontCollection::new(),
        }
    }

    /// `registerFont` adds font bytes under a family name.
    ///
    /// `bytes` is copied into the collection. Face zero of the file is parsed;
    /// a TrueType collection uses that first face. Returns `false` when the
    /// bytes cannot be parsed, and does not record a fallback. When `fallback`
    /// is true and registration succeeds, the face is also appended to the
    /// global fallback order used after requested families.
    #[wasm_bindgen(js_name = registerFont)]
    pub fn register_font(&mut self, family: &str, bytes: &[u8], fallback: bool) -> bool {
        let Some(identifier) = self.inner.register(family, bytes.to_vec()) else {
            return false;
        };
        if fallback {
            self.inner.add_fallback(identifier);
        }
        true
    }

    /// `isEmpty` reports whether no fonts are currently registered.
    #[wasm_bindgen(getter, js_name = isEmpty)]
    pub fn is_empty(&self) -> bool {
        self.inner.is_empty()
    }
}

impl Default for WebFontCollection {
    fn default() -> Self {
        Self::new()
    }
}

/// `WebParagraph` is laid-out text ready to draw or measure.
///
/// The constructor shapes the string, selects fonts from the collection, wraps
/// glyphs into lines, and positions them within `maxWidth`. Call [`Self::layout`]
/// again to wrap at another width without repeating shaping. Draw with
/// [`crate::WebDisplayListBuilder::draw_paragraph`].
#[wasm_bindgen(js_name = Paragraph)]
pub struct WebParagraph {
    pub(crate) inner: Paragraph,
    outline_bounds: Option<Option<valo::Rect>>,
}

#[wasm_bindgen(js_class = Paragraph)]
impl WebParagraph {
    /// `new` shapes and lays out one styled paragraph.
    ///
    /// Throws if `fonts` has no registered faces. `families` is a newline-separated
    /// fallback list; empty entries are ignored. `size` is the font size in
    /// logical pixels. `weight` is a CSS font weight clamped to `1..=65535`.
    /// `stretch` is CSS font-width as a percentage, where 100 is normal; Valo
    /// selects a registered width and does not synthesize one.
    ///
    /// Color components are straight-alpha sRGB. `kerning` enables the font's
    /// kerning adjustments. `variantCaps` is `0` normal through `6` titling
    /// caps; any other value uses normal. If a font lacks the requested
    /// feature, text remains unchanged.
    ///
    /// `letterSpacing` adds logical pixels after each grapheme cluster.
    /// `wordSpacing` adds logical pixels after each space, in addition to letter
    /// spacing. `lineHeight` greater than zero is a multiplier of `size`; zero
    /// or negative uses the font's metrics.
    ///
    /// `align` is `0` left, `1` center, `2` right, or `3` justify; any other
    /// value uses left. `direction` is `0` infer from the first strong
    /// character (Canvas2D `"inherit"`), `1` LTR, or `2` RTL; any other value
    /// infers. `maxLines` of zero means unlimited. An empty `ellipsis` omits
    /// truncation text. `maxWidth` is the layout width in logical pixels;
    /// `Infinity` disables soft wrapping. The constructor lays out once.
    #[allow(clippy::too_many_arguments)]
    #[wasm_bindgen(constructor)]
    pub fn new(
        fonts: &mut WebFontCollection,
        text: &str,
        families: &str,
        size: f32,
        weight: u32,
        italic: bool,
        red: f32,
        green: f32,
        blue: f32,
        alpha: f32,
        stretch: f32,
        kerning: bool,
        variant_caps: u32,
        letter_spacing: f32,
        word_spacing: f32,
        line_height: f32,
        align: u32,
        direction: u32,
        max_lines: u32,
        ellipsis: &str,
        max_width: f32,
        preserve_trailing_whitespace: bool,
    ) -> Result<WebParagraph, JsValue> {
        if fonts.inner.is_empty() {
            return Err(JsValue::from_str(
                "register at least one font before building text",
            ));
        }
        let style = TextStyle {
            families: families
                .split('\n')
                .filter(|family| !family.is_empty())
                .map(str::to_owned)
                .collect(),
            weight: weight.clamp(1, u16::MAX as u32) as u16,
            italic,
            stretch,
            kerning,
            variant_caps: variant_caps_of(variant_caps),
            size,
            color: Color::rgba(red, green, blue, alpha),
            letter_spacing,
            word_spacing,
            height: (line_height > 0.0).then_some(line_height),
            ..TextStyle::default()
        };
        let paragraph_style = ParagraphStyle {
            align: text_align(align),
            direction: text_direction(direction),
            preserve_trailing_whitespace,
            max_lines: (max_lines > 0).then_some(max_lines),
            ellipsis: (!ellipsis.is_empty()).then(|| ellipsis.to_owned()),
        };
        let mut builder = ParagraphBuilder::new(&mut fonts.inner);
        builder.style(paragraph_style).add_text(text, &style);
        let mut paragraph = builder.build();
        paragraph.layout(max_width);
        Ok(WebParagraph {
            inner: paragraph,
            outline_bounds: None,
        })
    }

    /// `layout` prepares the paragraph for drawing within `maxWidth`.
    ///
    /// It wraps the shaped glyphs into lines and computes their positions and
    /// metrics. Repeating the same width reuses the existing layout; shaping is
    /// never repeated. `Infinity` disables soft wrapping. Outline metrics are
    /// recomputed on the next read.
    pub fn layout(&mut self, max_width: f32) {
        self.inner.layout(max_width);
        self.outline_bounds = None;
    }

    /// `width` is the nonnegative width of the widest laid-out line, in logical pixels.
    #[wasm_bindgen(getter)]
    pub fn width(&self) -> f32 {
        self.inner.width()
    }

    /// `advance` is the pen's signed travel, which `width` floors at zero.
    ///
    /// Canvas2D's `TextMetrics.width` is an advance, not a layout box. Letter
    /// and word spacing tighter than the glyphs are wide walks the pen
    /// backwards.
    #[wasm_bindgen(getter)]
    pub fn advance(&self) -> f32 {
        self.inner.advance()
    }

    /// `lastGlyphOrigin` is the paragraph-local x origin of the final glyph.
    ///
    /// It is `undefined` when the paragraph placed none. The Canvas adapter
    /// needs somewhere to anchor the bounding box of text that puts down no
    /// ink at all.
    #[wasm_bindgen(getter, js_name = lastGlyphOrigin)]
    pub fn last_glyph_origin(&self) -> Option<f32> {
        self.inner.last_glyph_origin()
    }

    /// `height` is the laid-out paragraph height in logical pixels.
    #[wasm_bindgen(getter)]
    pub fn height(&self) -> f32 {
        self.inner.height()
    }

    /// `alphabeticBaseline` is the first line's baseline y in paragraph coordinates.
    ///
    /// It is zero when the paragraph has no lines.
    #[wasm_bindgen(getter, js_name = alphabeticBaseline)]
    pub fn alphabetic_baseline(&self) -> f32 {
        self.inner.lines().first().map_or(0.0, |line| line.baseline)
    }

    /// `firstLineAscent` is the first line's maximum distance above the baseline, in logical pixels.
    ///
    /// It is zero when the paragraph has no lines.
    #[wasm_bindgen(getter, js_name = firstLineAscent)]
    pub fn first_line_ascent(&self) -> f32 {
        self.inner.lines().first().map_or(0.0, |line| line.ascent)
    }

    /// `firstLineDescent` is the first line's maximum distance below the baseline, in logical pixels.
    ///
    /// It is zero when the paragraph has no lines.
    #[wasm_bindgen(getter, js_name = firstLineDescent)]
    pub fn first_line_descent(&self) -> f32 {
        self.inner.lines().first().map_or(0.0, |line| line.descent)
    }

    /// `outlineLeft` is the left edge of visible glyph ink in paragraph coordinates.
    ///
    /// It is zero when the paragraph has no visible glyphs. This query may
    /// rasterize color glyphs.
    #[wasm_bindgen(getter, js_name = outlineLeft)]
    pub fn outline_left(&mut self) -> f32 {
        self.outline_bounds().map_or(0.0, |bounds| bounds.x)
    }

    /// `outlineTop` is the top edge of visible glyph ink in paragraph coordinates.
    ///
    /// It is zero when the paragraph has no visible glyphs.
    #[wasm_bindgen(getter, js_name = outlineTop)]
    pub fn outline_top(&mut self) -> f32 {
        self.outline_bounds().map_or(0.0, |bounds| bounds.y)
    }

    /// `outlineRight` is the right edge of visible glyph ink in paragraph coordinates.
    ///
    /// It is zero when the paragraph has no visible glyphs.
    #[wasm_bindgen(getter, js_name = outlineRight)]
    pub fn outline_right(&mut self) -> f32 {
        self.outline_bounds().map_or(0.0, |bounds| bounds.right())
    }

    /// `outlineBottom` is the bottom edge of visible glyph ink in paragraph coordinates.
    ///
    /// It is zero when the paragraph has no visible glyphs.
    #[wasm_bindgen(getter, js_name = outlineBottom)]
    pub fn outline_bottom(&mut self) -> f32 {
        self.outline_bounds().map_or(0.0, |bounds| bounds.bottom())
    }

    /// `hasOutline` reports whether the paragraph has visible glyph ink.
    ///
    /// It is false for empty or whitespace-only text. This query may rasterize
    /// color glyphs.
    #[wasm_bindgen(getter, js_name = hasOutline)]
    pub fn has_outline(&mut self) -> bool {
        self.outline_bounds().is_some()
    }

    /// `primaryFontAscent` is the primary font's ascent at the paragraph size, in logical pixels.
    ///
    /// The primary font is the first run's face, or the first styled span when
    /// no glyphs were placed. It is zero when no font is available.
    #[wasm_bindgen(getter, js_name = primaryFontAscent)]
    pub fn primary_font_ascent(&self) -> f32 {
        primary_metrics(&self.inner).map_or(0.0, |(_, font, size)| font.ascent_px(size))
    }

    /// `primaryFontDescent` is the primary font's descent at the paragraph size, in logical pixels.
    ///
    /// It is zero when no font is available.
    #[wasm_bindgen(getter, js_name = primaryFontDescent)]
    pub fn primary_font_descent(&self) -> f32 {
        primary_metrics(&self.inner).map_or(0.0, |(_, font, size)| font.descent_px(size))
    }

    /// `emAscent` is the em-square portion above the baseline at the paragraph size.
    ///
    /// It scales the font's ascent so ascent plus descent equals `size`. It is
    /// zero when no font is available, except a zero line-height font reports
    /// `size` of ascent.
    #[wasm_bindgen(getter, js_name = emAscent)]
    pub fn em_ascent(&self) -> f32 {
        primary_metrics(&self.inner).map_or(0.0, |(_, font, size)| em_metrics(font, size).0)
    }

    /// `emDescent` is the em-square portion below the baseline at the paragraph size.
    ///
    /// It is zero when no font is available or when the font's line height is
    /// effectively zero.
    #[wasm_bindgen(getter, js_name = emDescent)]
    pub fn em_descent(&self) -> f32 {
        primary_metrics(&self.inner).map_or(0.0, |(_, font, size)| em_metrics(font, size).1)
    }

    /// `topBaselineOrigin` is the y offset that places the em-square top on the draw origin.
    ///
    /// It is zero when no font is available. Use it for Canvas2D `textBaseline = "top"`.
    #[wasm_bindgen(getter, js_name = topBaselineOrigin)]
    pub fn top_baseline_origin(&self) -> f32 {
        let Some((baseline, font, size)) = primary_metrics(&self.inner) else {
            return 0.0;
        };
        let (em_ascent, _) = em_metrics(font, size);
        -baseline + em_ascent
    }

    /// `hangingBaselineOffset` is how far above the alphabetic baseline the hanging baseline sits.
    ///
    /// Valo does not read the OpenType `BASE` table, so this is always 80% of
    /// the primary font's ascent. It is zero when no font is available.
    #[wasm_bindgen(getter, js_name = hangingBaselineOffset)]
    pub fn hanging_baseline_offset(&self) -> f32 {
        primary_metrics(&self.inner).map_or(0.0, |(_, font, size)| font.ascent_px(size) * 0.8)
    }

    /// `ideographicBaselineOffset` is how far above the alphabetic baseline the ideographic-under baseline sits.
    ///
    /// The value is negative because that baseline sits below the alphabetic
    /// baseline by the font's descent. It is zero when no font is available.
    #[wasm_bindgen(getter, js_name = ideographicBaselineOffset)]
    pub fn ideographic_baseline_offset(&self) -> f32 {
        primary_metrics(&self.inner).map_or(0.0, |(_, font, size)| -font.descent_px(size))
    }

    /// `hangingBaselineOrigin` is the y offset that places the hanging baseline on the draw origin.
    ///
    /// It is zero when no font is available.
    #[wasm_bindgen(getter, js_name = hangingBaselineOrigin)]
    pub fn hanging_baseline_origin(&self) -> f32 {
        self.baseline_origin(self.hanging_baseline_offset())
    }

    /// `middleBaselineOrigin` is the y offset that places the em-square midpoint on the draw origin.
    ///
    /// It is zero when no font is available.
    #[wasm_bindgen(getter, js_name = middleBaselineOrigin)]
    pub fn middle_baseline_origin(&self) -> f32 {
        let Some((baseline, font, size)) = primary_metrics(&self.inner) else {
            return 0.0;
        };
        let (em_ascent, em_descent) = em_metrics(font, size);
        -baseline + (em_ascent - em_descent) * 0.5
    }

    /// `bottomBaselineOrigin` is the y offset that places the em-square bottom on the draw origin.
    ///
    /// It is zero when no font is available.
    #[wasm_bindgen(getter, js_name = bottomBaselineOrigin)]
    pub fn bottom_baseline_origin(&self) -> f32 {
        let Some((baseline, font, size)) = primary_metrics(&self.inner) else {
            return 0.0;
        };
        let (_, em_descent) = em_metrics(font, size);
        -baseline - em_descent
    }

    /// `ideographicBaselineOrigin` is the y offset that places the ideographic-under baseline on the draw origin.
    ///
    /// It is zero when no font is available.
    #[wasm_bindgen(getter, js_name = ideographicBaselineOrigin)]
    pub fn ideographic_baseline_origin(&self) -> f32 {
        self.baseline_origin(self.ideographic_baseline_offset())
    }

    /// `minIntrinsicWidth` is the widest unbreakable segment, in logical pixels.
    #[wasm_bindgen(getter, js_name = minIntrinsicWidth)]
    pub fn min_intrinsic_width(&self) -> f32 {
        self.inner.min_intrinsic_width()
    }

    /// `maxIntrinsicWidth` is the width required to avoid soft wrapping, in logical pixels.
    #[wasm_bindgen(getter, js_name = maxIntrinsicWidth)]
    pub fn max_intrinsic_width(&self) -> f32 {
        self.inner.max_intrinsic_width()
    }

    /// `truncated` reports whether the line limit omitted content.
    #[wasm_bindgen(getter)]
    pub fn truncated(&self) -> bool {
        self.inner.truncated()
    }
}

impl WebParagraph {
    /// Where the paragraph's top goes so that a baseline `offset` above the
    /// alphabetic baseline lands on the draw origin.
    fn baseline_origin(&self, offset: f32) -> f32 {
        primary_metrics(&self.inner).map_or(0.0, |(baseline, _, _)| -baseline + offset)
    }

    fn outline_bounds(&mut self) -> Option<valo::Rect> {
        *self
            .outline_bounds
            .get_or_insert_with(|| self.inner.ink_bounds())
    }
}

fn primary_metrics(paragraph: &Paragraph) -> Option<(f32, &valo::Font, f32)> {
    let line = paragraph.lines().first()?;
    let (font, size) = paragraph.primary_font()?;
    Some((line.baseline, font, size))
}

fn em_metrics(font: &valo::Font, size: f32) -> (f32, f32) {
    let ascent = font.ascent_px(size);
    let descent = font.descent_px(size);
    let line_height = ascent + descent;
    if line_height <= f32::EPSILON {
        return (size, 0.0);
    }
    (size * ascent / line_height, size * descent / line_height)
}

/// 0 = infer from content (Canvas2D's `"inherit"`), 1 = ltr, 2 = rtl.
fn text_direction(value: u32) -> Option<TextDirection> {
    match value {
        1 => Some(TextDirection::Ltr),
        2 => Some(TextDirection::Rtl),
        _ => None,
    }
}

fn variant_caps_of(value: u32) -> VariantCaps {
    [
        VariantCaps::Normal,
        VariantCaps::SmallCaps,
        VariantCaps::AllSmallCaps,
        VariantCaps::PetiteCaps,
        VariantCaps::AllPetiteCaps,
        VariantCaps::Unicase,
        VariantCaps::TitlingCaps,
    ]
    .get(value as usize)
    .copied()
    .unwrap_or_default()
}

fn text_align(value: u32) -> TextAlign {
    [
        TextAlign::Left,
        TextAlign::Center,
        TextAlign::Right,
        TextAlign::Justify,
    ]
    .get(value as usize)
    .copied()
    .unwrap_or_default()
}
