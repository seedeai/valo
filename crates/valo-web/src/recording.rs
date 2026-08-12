use std::sync::Arc;

use valo::{DisplayList, DisplayListBuilder, DrawGlyphRunExt, DrawParagraphExt, Rect};
use wasm_bindgen::prelude::*;

use crate::path::{elliptical_radii, WebPath};
use crate::renderer::WebImage;
use crate::style::WebPaint;
use crate::text::WebParagraph;
use crate::types;

#[wasm_bindgen(js_name = DisplayList)]
pub struct WebDisplayList {
    pub(crate) inner: Arc<DisplayList>,
}

#[wasm_bindgen(js_class = DisplayList)]
impl WebDisplayList {
    #[wasm_bindgen(getter)]
    pub fn draw_count(&self) -> u32 {
        self.inner.draw_count()
    }
}

#[wasm_bindgen(js_name = DisplayListBuilder)]
pub struct WebDisplayListBuilder {
    inner: Option<DisplayListBuilder>,
}

impl WebDisplayListBuilder {
    fn builder(&mut self) -> Result<&mut DisplayListBuilder, JsValue> {
        self.inner
            .as_mut()
            .ok_or_else(|| JsValue::from_str("this display-list builder was already built"))
    }
}

#[wasm_bindgen(js_class = DisplayListBuilder)]
impl WebDisplayListBuilder {
    #[wasm_bindgen(constructor)]
    pub fn new() -> WebDisplayListBuilder {
        WebDisplayListBuilder {
            inner: Some(DisplayListBuilder::new()),
        }
    }

    pub fn save(&mut self) -> Result<(), JsValue> {
        self.builder()?.save();
        Ok(())
    }

    pub fn restore(&mut self) -> Result<(), JsValue> {
        self.builder()?.restore();
        Ok(())
    }

    #[wasm_bindgen(js_name = saveLayer)]
    pub fn save_layer(&mut self, paint: &WebPaint) -> Result<(), JsValue> {
        self.builder()?.save_layer(None, &paint.inner);
        Ok(())
    }

    #[allow(clippy::too_many_arguments)]
    #[wasm_bindgen(js_name = saveLayerBounds)]
    pub fn save_layer_bounds(
        &mut self,
        x: f32,
        y: f32,
        width: f32,
        height: f32,
        paint: &WebPaint,
    ) -> Result<(), JsValue> {
        self.builder()?
            .save_layer(Some(Rect::new(x, y, width, height)), &paint.inner);
        Ok(())
    }

    pub fn translate(&mut self, x: f32, y: f32) -> Result<(), JsValue> {
        self.builder()?.translate(x, y);
        Ok(())
    }

    pub fn scale(&mut self, x: f32, y: f32) -> Result<(), JsValue> {
        self.builder()?.scale(x, y);
        Ok(())
    }

    pub fn rotate(&mut self, radians: f32) -> Result<(), JsValue> {
        self.builder()?.rotate(radians);
        Ok(())
    }

    pub fn transform(&mut self, values: &[f32]) -> Result<(), JsValue> {
        self.builder()?.concat(&types::matrix(values)?);
        Ok(())
    }

    #[wasm_bindgen(js_name = clipRect)]
    pub fn clip_rect(
        &mut self,
        x: f32,
        y: f32,
        width: f32,
        height: f32,
        operation: u32,
    ) -> Result<(), JsValue> {
        self.builder()?
            .clip_rect(Rect::new(x, y, width, height), types::clip_op(operation));
        Ok(())
    }

    #[wasm_bindgen(js_name = clipPath)]
    pub fn clip_path(
        &mut self,
        path: &mut WebPath,
        fill_rule: u32,
        operation: u32,
    ) -> Result<(), JsValue> {
        self.builder()?.clip_path(
            &path.built(),
            types::fill_rule(fill_rule),
            types::clip_op(operation),
        );
        Ok(())
    }

    #[allow(clippy::too_many_arguments)]
    #[wasm_bindgen(js_name = drawRect)]
    pub fn draw_rect(
        &mut self,
        x: f32,
        y: f32,
        width: f32,
        height: f32,
        paint: &WebPaint,
    ) -> Result<(), JsValue> {
        self.builder()?
            .draw_rect(Rect::new(x, y, width, height), &paint.inner);
        Ok(())
    }

    #[allow(clippy::too_many_arguments)]
    #[wasm_bindgen(js_name = drawRoundedRect)]
    pub fn draw_rounded_rect(
        &mut self,
        x: f32,
        y: f32,
        width: f32,
        height: f32,
        radii: &[f32],
        paint: &WebPaint,
    ) -> Result<(), JsValue> {
        self.builder()?.draw_rrect_radii_elliptical(
            Rect::new(x, y, width, height),
            elliptical_radii(radii)?,
            &paint.inner,
        );
        Ok(())
    }

    #[wasm_bindgen(js_name = drawPath)]
    pub fn draw_path(
        &mut self,
        path: &mut WebPath,
        fill_rule: u32,
        paint: &WebPaint,
    ) -> Result<(), JsValue> {
        self.builder()?
            .draw_path(&path.built(), types::fill_rule(fill_rule), &paint.inner);
        Ok(())
    }

    #[allow(clippy::too_many_arguments)]
    #[wasm_bindgen(js_name = drawImageRect)]
    pub fn draw_image_rect(
        &mut self,
        image: &WebImage,
        source_x: f32,
        source_y: f32,
        source_width: f32,
        source_height: f32,
        destination_x: f32,
        destination_y: f32,
        destination_width: f32,
        destination_height: f32,
        filter: u32,
        tile_x: u32,
        tile_y: u32,
        paint: &WebPaint,
    ) -> Result<(), JsValue> {
        self.builder()?.draw_image_rect(
            &image.inner,
            Rect::new(source_x, source_y, source_width, source_height),
            Rect::new(
                destination_x,
                destination_y,
                destination_width,
                destination_height,
            ),
            types::sampling(filter, tile_x, tile_y),
            &paint.inner,
        );
        Ok(())
    }

    #[wasm_bindgen(js_name = drawDisplayList)]
    pub fn draw_display_list(&mut self, list: &WebDisplayList) -> Result<(), JsValue> {
        self.builder()?.draw_display_list(&list.inner);
        Ok(())
    }

    #[wasm_bindgen(js_name = drawDisplayListCached)]
    pub fn draw_display_list_cached(&mut self, list: &WebDisplayList) -> Result<(), JsValue> {
        self.builder()?.draw_display_list_cached(&list.inner);
        Ok(())
    }

    #[wasm_bindgen(js_name = drawParagraph)]
    pub fn draw_paragraph(
        &mut self,
        paragraph: &WebParagraph,
        x: f32,
        y: f32,
    ) -> Result<(), JsValue> {
        self.builder()?.draw_paragraph(&paragraph.inner, (x, y));
        Ok(())
    }

    #[wasm_bindgen(js_name = drawParagraphWith)]
    pub fn draw_paragraph_with(
        &mut self,
        paragraph: &WebParagraph,
        x: f32,
        y: f32,
        paint: &WebPaint,
    ) -> Result<(), JsValue> {
        self.builder()?
            .draw_paragraph_with(&paragraph.inner, (x, y), &paint.inner);
        Ok(())
    }

    #[wasm_bindgen(js_name = backdropBlur)]
    pub fn backdrop_blur(
        &mut self,
        x: f32,
        y: f32,
        width: f32,
        height: f32,
        sigma: f32,
    ) -> Result<(), JsValue> {
        self.builder()?
            .backdrop_blur(Rect::new(x, y, width, height), sigma);
        Ok(())
    }

    pub fn build(&mut self) -> Result<WebDisplayList, JsValue> {
        let builder = self
            .inner
            .take()
            .ok_or_else(|| JsValue::from_str("this display-list builder was already built"))?;
        Ok(WebDisplayList {
            inner: Arc::new(builder.build()),
        })
    }
}

impl Default for WebDisplayListBuilder {
    fn default() -> Self {
        Self::new()
    }
}
