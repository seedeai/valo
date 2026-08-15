use valo::{
    Color, FontCollection, Paragraph, ParagraphBuilder, ParagraphStyle, TextAlign, TextDirection,
    TextStyle, VariantCaps,
};
use wasm_bindgen::prelude::*;

#[wasm_bindgen(js_name = FontCollection)]
pub struct WebFontCollection {
    pub(crate) inner: FontCollection,
}

#[wasm_bindgen(js_class = FontCollection)]
impl WebFontCollection {
    #[wasm_bindgen(constructor)]
    pub fn new() -> WebFontCollection {
        WebFontCollection {
            inner: FontCollection::new(),
        }
    }

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

#[wasm_bindgen(js_name = Paragraph)]
pub struct WebParagraph {
    pub(crate) inner: Paragraph,
    outline_bounds: Option<Option<valo::Rect>>,
}

#[wasm_bindgen(js_class = Paragraph)]
impl WebParagraph {
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

    pub fn layout(&mut self, max_width: f32) {
        self.inner.layout(max_width);
        self.outline_bounds = None;
    }

    #[wasm_bindgen(getter)]
    pub fn width(&self) -> f32 {
        self.inner.width()
    }

    #[wasm_bindgen(getter)]
    pub fn height(&self) -> f32 {
        self.inner.height()
    }

    #[wasm_bindgen(getter, js_name = alphabeticBaseline)]
    pub fn alphabetic_baseline(&self) -> f32 {
        self.inner.lines().first().map_or(0.0, |line| line.baseline)
    }

    #[wasm_bindgen(getter, js_name = firstLineAscent)]
    pub fn first_line_ascent(&self) -> f32 {
        self.inner.lines().first().map_or(0.0, |line| line.ascent)
    }

    #[wasm_bindgen(getter, js_name = firstLineDescent)]
    pub fn first_line_descent(&self) -> f32 {
        self.inner.lines().first().map_or(0.0, |line| line.descent)
    }

    #[wasm_bindgen(getter, js_name = outlineLeft)]
    pub fn outline_left(&mut self) -> f32 {
        self.outline_bounds().map_or(0.0, |bounds| bounds.x)
    }

    #[wasm_bindgen(getter, js_name = outlineTop)]
    pub fn outline_top(&mut self) -> f32 {
        self.outline_bounds().map_or(0.0, |bounds| bounds.y)
    }

    #[wasm_bindgen(getter, js_name = outlineRight)]
    pub fn outline_right(&mut self) -> f32 {
        self.outline_bounds().map_or(0.0, |bounds| bounds.right())
    }

    #[wasm_bindgen(getter, js_name = outlineBottom)]
    pub fn outline_bottom(&mut self) -> f32 {
        self.outline_bounds().map_or(0.0, |bounds| bounds.bottom())
    }

    #[wasm_bindgen(getter, js_name = hasOutline)]
    pub fn has_outline(&mut self) -> bool {
        self.outline_bounds().is_some()
    }

    #[wasm_bindgen(getter, js_name = primaryFontAscent)]
    pub fn primary_font_ascent(&self) -> f32 {
        primary_metrics(&self.inner).map_or(0.0, |(_, font, size)| font.ascent_px(size))
    }

    #[wasm_bindgen(getter, js_name = primaryFontDescent)]
    pub fn primary_font_descent(&self) -> f32 {
        primary_metrics(&self.inner).map_or(0.0, |(_, font, size)| font.descent_px(size))
    }

    #[wasm_bindgen(getter, js_name = emAscent)]
    pub fn em_ascent(&self) -> f32 {
        primary_metrics(&self.inner).map_or(0.0, |(_, font, size)| em_metrics(font, size).0)
    }

    #[wasm_bindgen(getter, js_name = emDescent)]
    pub fn em_descent(&self) -> f32 {
        primary_metrics(&self.inner).map_or(0.0, |(_, font, size)| em_metrics(font, size).1)
    }

    #[wasm_bindgen(getter, js_name = topBaselineOrigin)]
    pub fn top_baseline_origin(&self) -> f32 {
        let Some((baseline, font, size)) = primary_metrics(&self.inner) else {
            return 0.0;
        };
        let (em_ascent, _) = em_metrics(font, size);
        -baseline + em_ascent
    }

    /// How far ABOVE the alphabetic baseline the hanging baseline sits.
    /// valo does not read the OpenType `BASE` table, so this is always
    /// Skia's fallback approximation for a font that lacks one.
    #[wasm_bindgen(getter, js_name = hangingBaselineOffset)]
    pub fn hanging_baseline_offset(&self) -> f32 {
        primary_metrics(&self.inner).map_or(0.0, |(_, font, size)| font.ascent_px(size) * 0.8)
    }

    /// How far above the alphabetic baseline the ideographic-under baseline
    /// sits — negative, since it sits below by the font's descent.
    #[wasm_bindgen(getter, js_name = ideographicBaselineOffset)]
    pub fn ideographic_baseline_offset(&self) -> f32 {
        primary_metrics(&self.inner).map_or(0.0, |(_, font, size)| -font.descent_px(size))
    }

    #[wasm_bindgen(getter, js_name = hangingBaselineOrigin)]
    pub fn hanging_baseline_origin(&self) -> f32 {
        self.baseline_origin(self.hanging_baseline_offset())
    }

    #[wasm_bindgen(getter, js_name = middleBaselineOrigin)]
    pub fn middle_baseline_origin(&self) -> f32 {
        let Some((baseline, font, size)) = primary_metrics(&self.inner) else {
            return 0.0;
        };
        let (em_ascent, em_descent) = em_metrics(font, size);
        -baseline + (em_ascent - em_descent) * 0.5
    }

    #[wasm_bindgen(getter, js_name = bottomBaselineOrigin)]
    pub fn bottom_baseline_origin(&self) -> f32 {
        let Some((baseline, font, size)) = primary_metrics(&self.inner) else {
            return 0.0;
        };
        let (_, em_descent) = em_metrics(font, size);
        -baseline - em_descent
    }

    #[wasm_bindgen(getter, js_name = ideographicBaselineOrigin)]
    pub fn ideographic_baseline_origin(&self) -> f32 {
        self.baseline_origin(self.ideographic_baseline_offset())
    }

    #[wasm_bindgen(getter, js_name = minIntrinsicWidth)]
    pub fn min_intrinsic_width(&self) -> f32 {
        self.inner.min_intrinsic_width()
    }

    #[wasm_bindgen(getter, js_name = maxIntrinsicWidth)]
    pub fn max_intrinsic_width(&self) -> f32 {
        self.inner.max_intrinsic_width()
    }

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
