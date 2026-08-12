use valo::{
    Color, FontCollection, Paragraph, ParagraphBuilder, ParagraphStyle, TextAlign, TextStyle,
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
        letter_spacing: f32,
        word_spacing: f32,
        line_height: f32,
        align: u32,
        max_lines: u32,
        ellipsis: &str,
        max_width: f32,
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
            size,
            color: Color::rgba(red, green, blue, alpha),
            letter_spacing,
            word_spacing,
            height: (line_height > 0.0).then_some(line_height),
            ..TextStyle::default()
        };
        let paragraph_style = ParagraphStyle {
            align: text_align(align),
            max_lines: (max_lines > 0).then_some(max_lines),
            ellipsis: (!ellipsis.is_empty()).then(|| ellipsis.to_owned()),
        };
        let mut builder = ParagraphBuilder::new(&mut fonts.inner);
        builder.style(paragraph_style).add_text(text, &style);
        let mut paragraph = builder.build();
        paragraph.layout(max_width);
        Ok(WebParagraph { inner: paragraph })
    }

    pub fn layout(&mut self, max_width: f32) {
        self.inner.layout(max_width);
    }

    #[wasm_bindgen(getter)]
    pub fn width(&self) -> f32 {
        self.inner.width()
    }

    #[wasm_bindgen(getter)]
    pub fn height(&self) -> f32 {
        self.inner.height()
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
