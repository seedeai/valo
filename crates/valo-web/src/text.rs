use std::sync::Arc;

use valo::{
    Color, Decoration, DecorationKind, Font, FontCollection, Paragraph, ParagraphBuilder,
    ParagraphStyle, Point, Shadow, TextAlign, TextDirection, TextStyle, VariantCaps,
};
use wasm_bindgen::prelude::*;

/// `WebRect` is an axis-aligned rectangle in logical pixels, y downward.
#[wasm_bindgen(js_name = Rect)]
#[derive(Clone, Copy, Default)]
pub struct WebRect {
    /// `x` is the left edge.
    pub x: f32,
    /// `y` is the top edge.
    pub y: f32,
    /// `width` is the horizontal extent.
    pub width: f32,
    /// `height` is the vertical extent.
    pub height: f32,
}

/// `WebTextRange` is a UTF-8 byte range `[start, end)` in paragraph text.
#[wasm_bindgen(js_name = TextRange)]
#[derive(Clone, Copy, Default)]
pub struct WebTextRange {
    /// `start` is the inclusive UTF-8 byte offset.
    pub start: u32,
    /// `end` is the exclusive UTF-8 byte offset.
    pub end: u32,
}

/// `WebTextPosition` is a caret placement in editable text.
#[wasm_bindgen(js_name = TextPosition)]
#[derive(Clone, Copy)]
pub struct WebTextPosition {
    /// `offset` is a UTF-8 byte offset in paragraph text.
    pub offset: u32,
    /// `downstream` selects the text after the offset when true.
    pub downstream: bool,
}

/// `WebLineMetrics` measures one laid-out line in paragraph-local logical pixels.
#[wasm_bindgen(js_name = LineMetrics)]
#[derive(Clone, Copy, Default)]
pub struct WebLineMetrics {
    /// `start` is the line's inclusive UTF-8 byte offset.
    pub start: u32,
    /// `end` is the line's exclusive UTF-8 byte offset.
    pub end: u32,
    /// `baseline` is the paragraph-local y coordinate of the baseline.
    pub baseline: f32,
    /// `ascent` is the distance above the baseline.
    pub ascent: f32,
    /// `descent` is the distance below the baseline.
    pub descent: f32,
    /// `left` is the paragraph-local x after alignment.
    pub left: f32,
    /// `width` is the line's signed content advance.
    pub width: f32,
}

fn from_rect(rect: valo::Rect) -> WebRect {
    WebRect {
        x: rect.x,
        y: rect.y,
        width: rect.width,
        height: rect.height,
    }
}

fn web_line_metrics(line: &valo::Line) -> WebLineMetrics {
    WebLineMetrics {
        start: byte_offset(line.range.start),
        end: byte_offset(line.range.end),
        baseline: line.baseline,
        ascent: line.ascent,
        descent: line.descent,
        left: line.left,
        width: line.width,
    }
}

fn byte_offset(offset: usize) -> u32 {
    offset.min(u32::MAX as usize) as u32
}

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

    /// `registerInstances` registers every face a font file offers.
    ///
    /// A static font is one face. A variable font contributes its named
    /// instances, so weights and styles select like a static family. When
    /// `fallback` is true, each added face is also appended to the global
    /// fallback order. Returns the number of faces added, or `0` when the
    /// bytes cannot be parsed.
    #[wasm_bindgen(js_name = registerInstances)]
    pub fn register_instances(&mut self, bytes: &[u8], fallback: bool) -> u32 {
        let instances = Font::instances_from_data(Arc::new(bytes.to_vec()), 0);
        let count = instances.len() as u32;
        for font in instances {
            let identifier = self.inner.add(font);
            if fallback {
                self.inner.add_fallback(identifier);
            }
        }
        count
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

/// `WebTextStyle` is font selection and paint for one span of text.
#[wasm_bindgen(js_name = TextStyle)]
#[derive(Clone)]
pub struct WebTextStyle {
    inner: TextStyle,
}

#[wasm_bindgen(js_class = TextStyle)]
impl WebTextStyle {
    /// `new` creates a style with one preferred family and size.
    ///
    /// Color defaults to opaque black. Call [`Self::set_color`] to change it.
    #[wasm_bindgen(constructor)]
    pub fn new(family: &str, size: f32) -> WebTextStyle {
        WebTextStyle {
            inner: TextStyle::new(family, size, Color::BLACK),
        }
    }

    /// `setColor` sets the span's fill as straight-alpha sRGB.
    #[wasm_bindgen(js_name = setColor)]
    pub fn set_color(&mut self, red: f32, green: f32, blue: f32, alpha: f32) {
        self.inner.color = Color::rgba(red, green, blue, alpha);
    }

    /// `setWeight` selects a CSS font weight, clamped to `1..=65535`.
    #[wasm_bindgen(js_name = setWeight)]
    pub fn set_weight(&mut self, weight: u32) {
        self.inner.weight = weight.clamp(1, u16::MAX as u32) as u16;
    }

    /// `setItalic` selects an italic face when one is registered.
    #[wasm_bindgen(js_name = setItalic)]
    pub fn set_italic(&mut self, italic: bool) {
        self.inner.italic = italic;
    }

    /// `setFamilies` replaces the preferred family list.
    ///
    /// `families` is newline-separated; empty entries are ignored. An empty
    /// list uses only the collection's fallbacks.
    #[wasm_bindgen(js_name = setFamilies)]
    pub fn set_families(&mut self, families: &str) {
        self.inner.families = families
            .split('\n')
            .filter(|family| !family.is_empty())
            .map(str::to_owned)
            .collect();
    }

    /// `setStretch` selects CSS font-width as a percentage, where 100 is normal.
    #[wasm_bindgen(js_name = setStretch)]
    pub fn set_stretch(&mut self, stretch: f32) {
        self.inner.stretch = stretch;
    }

    /// `setKerning` enables the font's kerning adjustments.
    #[wasm_bindgen(js_name = setKerning)]
    pub fn set_kerning(&mut self, kerning: bool) {
        self.inner.kerning = kerning;
    }

    /// `setVariantCaps` is `0` normal through `6` titling caps.
    ///
    /// Any other value uses normal. If a font lacks the feature, text is unchanged.
    #[wasm_bindgen(js_name = setVariantCaps)]
    pub fn set_variant_caps(&mut self, variant_caps: u32) {
        self.inner.variant_caps = variant_caps_of(variant_caps);
    }

    /// `setLetterSpacing` adds logical pixels after each grapheme cluster.
    #[wasm_bindgen(js_name = setLetterSpacing)]
    pub fn set_letter_spacing(&mut self, letter_spacing: f32) {
        self.inner.letter_spacing = letter_spacing;
    }

    /// `setWordSpacing` adds logical pixels after each space, in addition to letter spacing.
    #[wasm_bindgen(js_name = setWordSpacing)]
    pub fn set_word_spacing(&mut self, word_spacing: f32) {
        self.inner.word_spacing = word_spacing;
    }

    /// `setLineHeight` is a multiplier of `size`.
    ///
    /// Zero or negative uses the font's metrics.
    #[wasm_bindgen(js_name = setLineHeight)]
    pub fn set_line_height(&mut self, line_height: f32) {
        self.inner.height = (line_height > 0.0).then_some(line_height);
    }

    /// `setDecoration` is `-1` none, `0` underline, `1` line-through, or `2` overline.
    ///
    /// Any other value removes the decoration. Color inherits the text color
    /// until [`Self::set_decoration_color`].
    #[wasm_bindgen(js_name = setDecoration)]
    pub fn set_decoration(&mut self, kind: i32) {
        self.inner.decoration = decoration_kind(kind).map(Decoration::new);
    }

    /// `setDecorationColor` recolours the current decoration.
    ///
    /// Alpha of `0` or less inherits the text color. It is a no-op without a
    /// decoration.
    #[wasm_bindgen(js_name = setDecorationColor)]
    pub fn set_decoration_color(&mut self, red: f32, green: f32, blue: f32, alpha: f32) {
        let Some(decoration) = self.inner.decoration.as_mut() else {
            return;
        };
        decoration.color = (alpha > 0.0).then_some(Color::rgba(red, green, blue, alpha));
    }

    /// `setDecorationThickness` multiplies the font's suggested decoration thickness.
    ///
    /// Zero or negative uses `1`. It is a no-op without a decoration.
    #[wasm_bindgen(js_name = setDecorationThickness)]
    pub fn set_decoration_thickness(&mut self, thickness: f32) {
        let Some(decoration) = self.inner.decoration.as_mut() else {
            return;
        };
        decoration.thickness = if thickness > 0.0 { thickness } else { 1.0 };
    }

    /// `addShadow` paints an offset copy beneath the text.
    ///
    /// `blur` is the Gaussian sigma; zero is a sharp copy.
    #[allow(clippy::too_many_arguments)]
    #[wasm_bindgen(js_name = addShadow)]
    pub fn add_shadow(
        &mut self,
        offset_x: f32,
        offset_y: f32,
        blur: f32,
        red: f32,
        green: f32,
        blue: f32,
        alpha: f32,
    ) {
        self.inner.shadows.push(Shadow {
            color: Color::rgba(red, green, blue, alpha),
            offset: Point::new(offset_x, offset_y),
            blur,
        });
    }

    /// `clearShadows` removes every text shadow.
    #[wasm_bindgen(js_name = clearShadows)]
    pub fn clear_shadows(&mut self) {
        self.inner.shadows.clear();
    }
}

/// `WebParagraphBuilder` accumulates styled spans until [`Self::build`].
///
/// The font collection is borrowed only at build. wasm-bindgen cannot hold a
/// Rust reference to another wasm object, so this matches the C API rather than
/// the native `ParagraphBuilder` lifetime.
#[wasm_bindgen(js_name = ParagraphBuilder)]
pub struct WebParagraphBuilder {
    style: ParagraphStyle,
    spans: Vec<(String, TextStyle)>,
}

#[wasm_bindgen(js_class = ParagraphBuilder)]
impl WebParagraphBuilder {
    /// `new` creates an empty builder with the default paragraph style.
    #[wasm_bindgen(constructor)]
    pub fn new() -> WebParagraphBuilder {
        WebParagraphBuilder {
            style: ParagraphStyle::default(),
            spans: Vec::new(),
        }
    }

    /// `setAlign` is `0` left, `1` center, `2` right, or `3` justify.
    ///
    /// Any other value uses left.
    #[wasm_bindgen(js_name = setAlign)]
    pub fn set_align(&mut self, align: u32) {
        self.style.align = text_align(align);
    }

    /// `setDirection` is `0` infer from content, `1` LTR, or `2` RTL.
    ///
    /// Any other value infers from the first strong character.
    #[wasm_bindgen(js_name = setDirection)]
    pub fn set_direction(&mut self, direction: u32) {
        self.style.direction = text_direction(direction);
    }

    /// `setMaxLines` limits laid-out lines. Zero means unlimited.
    #[wasm_bindgen(js_name = setMaxLines)]
    pub fn set_max_lines(&mut self, max_lines: u32) {
        self.style.max_lines = (max_lines > 0).then_some(max_lines);
    }

    /// `setEllipsis` replaces omitted content when the line limit truncates.
    ///
    /// An empty string omits truncation text.
    #[wasm_bindgen(js_name = setEllipsis)]
    pub fn set_ellipsis(&mut self, ellipsis: &str) {
        self.style.ellipsis = (!ellipsis.is_empty()).then(|| ellipsis.to_owned());
    }

    /// `setPreserveTrailingWhitespace` includes trailing spaces in line widths.
    #[wasm_bindgen(js_name = setPreserveTrailingWhitespace)]
    pub fn set_preserve_trailing_whitespace(&mut self, preserve: bool) {
        self.style.preserve_trailing_whitespace = preserve;
    }

    /// `addText` appends a UTF-8 span with its own style.
    #[wasm_bindgen(js_name = addText)]
    pub fn add_text(&mut self, text: &str, style: &WebTextStyle) {
        self.spans.push((text.to_owned(), style.inner.clone()));
    }

    /// `build` shapes the accumulated spans against `fonts`.
    ///
    /// Throws if `fonts` has no registered faces. Call [`WebParagraph::layout`]
    /// before drawing. The builder is emptied and can be reused.
    pub fn build(&mut self, fonts: &mut WebFontCollection) -> Result<WebParagraph, JsValue> {
        let paragraph = shape_spans(fonts, &self.style, &self.spans)?;
        self.spans.clear();
        Ok(paragraph)
    }
}

impl Default for WebParagraphBuilder {
    fn default() -> Self {
        Self::new()
    }
}

fn shape_spans(
    fonts: &mut WebFontCollection,
    style: &ParagraphStyle,
    spans: &[(String, TextStyle)],
) -> Result<WebParagraph, JsValue> {
    if fonts.inner.is_empty() {
        return Err(JsValue::from_str(
            "register at least one font before building text",
        ));
    }
    let mut builder = ParagraphBuilder::new(&mut fonts.inner);
    builder.style(style.clone());
    for (text, span_style) in spans {
        builder.add_text(text, span_style);
    }
    Ok(WebParagraph {
        inner: builder.build(),
        outline_bounds: None,
    })
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
    /// This is a one-span convenience used by the Canvas2D adapter. New code
    /// should use [`WebParagraphBuilder`] and [`WebTextStyle`], which name each
    /// field instead of taking this argument list. The constructor still lays
    /// out once; `maxWidth` of `Infinity` disables soft wrapping.
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
        let mut paragraph = shape_spans(fonts, &paragraph_style, &[(text.to_owned(), style)])?;
        paragraph.layout(max_width);
        Ok(paragraph)
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

    /// `text` is the complete UTF-8 paragraph string.
    #[wasm_bindgen(getter)]
    pub fn text(&self) -> String {
        self.inner.text().to_owned()
    }

    /// `longestLine` is the signed advance of the widest laid-out line.
    ///
    /// It is zero before layout.
    #[wasm_bindgen(getter, js_name = longestLine)]
    pub fn longest_line(&self) -> f32 {
        self.inner.longest_line()
    }

    /// `lineCount` is the number of laid-out lines.
    ///
    /// It is zero before layout.
    #[wasm_bindgen(getter, js_name = lineCount)]
    pub fn line_count(&self) -> u32 {
        byte_offset(self.inner.lines().len())
    }

    /// `caretForOffset` returns a zero-width caret rectangle for a UTF-8 offset.
    ///
    /// The rectangle is paragraph-local. It is a zero rect before layout or
    /// when no line exists.
    #[wasm_bindgen(js_name = caretForOffset)]
    pub fn caret_for_offset(&self, offset: u32) -> WebRect {
        from_rect(self.inner.caret_for_offset(offset as usize))
    }

    /// `glyphPositionAt` maps a paragraph-local point to a caret position.
    ///
    /// An empty or unlaid-out paragraph returns offset `0` with downstream
    /// affinity.
    #[wasm_bindgen(js_name = glyphPositionAt)]
    pub fn glyph_position_at(&self, x: f32, y: f32) -> WebTextPosition {
        let position = self.inner.glyph_position_at(Point::new(x, y));
        WebTextPosition {
            offset: byte_offset(position.offset),
            downstream: position.downstream,
        }
    }

    /// `rectsForRange` returns selection boxes for a UTF-8 byte range.
    ///
    /// Bidirectional text may produce several boxes on one line.
    #[wasm_bindgen(js_name = rectsForRange)]
    pub fn rects_for_range(&self, start: u32, end: u32) -> Vec<WebRect> {
        self.inner
            .rects_for_range((start as usize)..(end as usize))
            .into_iter()
            .map(from_rect)
            .collect()
    }

    /// `wordBoundary` returns the word range around a UTF-8 byte offset.
    #[wasm_bindgen(js_name = wordBoundary)]
    pub fn word_boundary(&self, offset: u32) -> WebTextRange {
        let range = self.inner.word_boundary(offset as usize);
        WebTextRange {
            start: byte_offset(range.start),
            end: byte_offset(range.end),
        }
    }

    /// `lineMetrics` returns measurements for one laid-out line.
    ///
    /// It is `undefined` before layout or past the last line.
    #[wasm_bindgen(js_name = lineMetrics)]
    pub fn line_metrics(&self, index: u32) -> Option<WebLineMetrics> {
        self.inner.lines().get(index as usize).map(web_line_metrics)
    }

    /// `demandFamilies` is a newline-separated list of families with no registered face.
    #[wasm_bindgen(getter, js_name = demandFamilies)]
    pub fn demand_families(&self) -> String {
        self.inner.demand().families.join("\n")
    }

    /// `demandCodepoints` is the uncovered Unicode scalar values.
    #[wasm_bindgen(getter, js_name = demandCodepoints)]
    pub fn demand_codepoints(&self) -> Vec<u32> {
        self.inner
            .demand()
            .codepoints
            .iter()
            .map(|(ch, _)| *ch as u32)
            .collect()
    }

    /// `updateColor` recolours one added span without reshaping.
    ///
    /// An out-of-range span index has no effect.
    #[wasm_bindgen(js_name = updateColor)]
    pub fn update_color(&mut self, span: u32, red: f32, green: f32, blue: f32, alpha: f32) {
        self.inner
            .update_color(span as usize, Color::rgba(red, green, blue, alpha));
        self.outline_bounds = None;
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

fn decoration_kind(value: i32) -> Option<DecorationKind> {
    match value {
        0 => Some(DecorationKind::Underline),
        1 => Some(DecorationKind::LineThrough),
        2 => Some(DecorationKind::Overline),
        _ => None,
    }
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
