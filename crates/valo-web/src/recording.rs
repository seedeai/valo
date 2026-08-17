use std::sync::Arc;

use valo::{DisplayList, DisplayListBuilder, DrawGlyphRunExt, DrawParagraphExt, Rect};
use wasm_bindgen::prelude::*;

use crate::path::{elliptical_radii, WebPath};
use crate::renderer::WebImage;
use crate::style::WebPaint;
use crate::text::WebParagraph;
use crate::types;

/// `WebDisplayList` is an immutable recording of drawing commands.
///
/// Display lists are GPU-free and nestable. Replay them with
/// [`crate::WebRenderer::render`] or nest them through
/// [`WebDisplayListBuilder::draw_display_list`]. Later edits to the builder
/// that produced the list do not affect it.
#[wasm_bindgen(js_name = DisplayList)]
pub struct WebDisplayList {
    pub(crate) inner: Arc<DisplayList>,
}

#[wasm_bindgen(js_class = DisplayList)]
impl WebDisplayList {
    /// `drawCount` is the number of draws, including nested lists.
    #[wasm_bindgen(getter)]
    pub fn draw_count(&self) -> u32 {
        self.inner.draw_count()
    }
}

/// `WebDisplayListBuilder` records drawing commands into an immutable display list.
///
/// Recording is GPU-free. The builder resolves bounds, clips, and layer extents
/// so rendering does not rediscover them. Every mutating method throws after
/// [`Self::build`]. Coordinates are local pixels with y downward until a
/// transform says otherwise.
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
    /// `new` creates an empty display-list builder.
    #[wasm_bindgen(constructor)]
    pub fn new() -> WebDisplayListBuilder {
        WebDisplayListBuilder {
            inner: Some(DisplayListBuilder::new()),
        }
    }

    /// `save` preserves the current transform and clip until the matching `restore`.
    pub fn save(&mut self) -> Result<(), JsValue> {
        self.builder()?.save();
        Ok(())
    }

    /// `restore` closes the most recent save or layer scope.
    ///
    /// An unmatched restore triggers a debug assertion and is ignored in
    /// release builds.
    pub fn restore(&mut self) -> Result<(), JsValue> {
        self.builder()?.restore();
        Ok(())
    }

    /// `saveLayer` begins an offscreen layer composited with `paint` at `restore`.
    ///
    /// Layer bounds are derived from the recorded children and the active clip.
    #[wasm_bindgen(js_name = saveLayer)]
    pub fn save_layer(&mut self, paint: &WebPaint) -> Result<(), JsValue> {
        self.builder()?.save_layer(None, &paint.inner);
        Ok(())
    }

    /// `saveLayerBounds` begins an offscreen layer cropped to a local-space rectangle.
    ///
    /// `x`, `y`, `width`, and `height` are a crop, not merely an allocation hint;
    /// content outside the rectangle is discarded. The layer composites with
    /// `paint` at `restore`.
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

    /// `translate` offsets subsequent drawing and clipping operations.
    pub fn translate(&mut self, x: f32, y: f32) -> Result<(), JsValue> {
        self.builder()?.translate(x, y);
        Ok(())
    }

    /// `scale` scales subsequent drawing and clipping operations.
    pub fn scale(&mut self, x: f32, y: f32) -> Result<(), JsValue> {
        self.builder()?.scale(x, y);
        Ok(())
    }

    /// `rotate` rotates subsequent drawing and clipping operations clockwise.
    ///
    /// `radians` is measured from +x. Positive angles rotate clockwise in
    /// Valo's y-down coordinate system.
    pub fn rotate(&mut self, radians: f32) -> Result<(), JsValue> {
        self.builder()?.rotate(radians);
        Ok(())
    }

    /// `transform` appends a transform for subsequent drawing and clipping operations.
    ///
    /// `values` must be 6 affine numbers `[a, b, c, d, tx, ty]` or 16
    /// column-major matrix values. Any other length throws.
    pub fn transform(&mut self, values: &[f32]) -> Result<(), JsValue> {
        self.builder()?.concat(&types::matrix(values)?);
        Ok(())
    }

    /// `clipRect` applies a rectangular clip until the current scope ends.
    ///
    /// `operation` is `0` intersect or `1` difference; any other value uses
    /// intersect.
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

    /// `clipPath` applies a path clip until the current scope ends.
    ///
    /// `fillRule` is `0` nonzero (and any value other than `1`) or `1` even-odd.
    /// `operation` is `0` intersect or `1` difference; any other value uses
    /// intersect.
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

    /// `drawRect` records a filled or stroked rectangle.
    ///
    /// Fill versus stroke comes from `paint`.
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

    /// `drawRoundedRect` records a rounded rectangle with circular or elliptical corners.
    ///
    /// `radii` must contain 1, 4, or 8 values: one radius for every corner, four
    /// circular radii clockwise from the top-left, or eight elliptical
    /// `[x, y]` pairs in that same corner order. Any other length throws.
    /// Adjacent radii that would overlap are proportionally reduced.
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

    /// `drawPath` records a filled or stroked path.
    ///
    /// `fillRule` is `0` nonzero (and any value other than `1`) or `1` even-odd.
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

    /// `drawImageRect` records a source image region into a destination rectangle.
    ///
    /// `sourceX`/`sourceY`/`sourceWidth`/`sourceHeight` are in source pixels.
    /// The destination is in local coordinates. Tiling applies when the source
    /// rectangle extends beyond the image.
    ///
    /// `filter` is `0` linear or `1` nearest; any other value uses linear.
    /// `mipmap` is `0` none, `1` nearest, or `2` linear; any other value uses
    /// linear. `tileX` and `tileY` are `0` clamp, `1` repeat, `2` mirror, or
    /// `3` decal; any other value uses clamp.
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
        mipmap: u32,
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
            types::sampling(filter, mipmap, tile_x, tile_y),
            &paint.inner,
        );
        Ok(())
    }

    /// `drawDisplayList` records a nested display list by shared reference.
    ///
    /// Later changes to `list` are impossible because a display list is
    /// immutable; later layout of a paragraph recorded into `list` does not
    /// affect this embedding.
    #[wasm_bindgen(js_name = drawDisplayList)]
    pub fn draw_display_list(&mut self, list: &WebDisplayList) -> Result<(), JsValue> {
        self.builder()?.draw_display_list(&list.inner);
        Ok(())
    }

    /// `drawDisplayListCached` records a nested list as a raster-cache candidate.
    ///
    /// Use it for stable, repeatedly drawn lists whose rendering is expensive.
    /// The renderer may still replay the list directly when caching is unsuitable.
    #[wasm_bindgen(js_name = drawDisplayListCached)]
    pub fn draw_display_list_cached(&mut self, list: &WebDisplayList) -> Result<(), JsValue> {
        self.builder()?.draw_display_list_cached(&list.inner);
        Ok(())
    }

    /// `drawParagraph` records the paragraph's current layout at a top-left origin.
    ///
    /// Glyph runs, shadows, and decorations are lowered into the list. Later
    /// layout or style changes do not affect the recorded commands. Call
    /// [`crate::WebParagraph`] layout before drawing; the paragraph constructor already
    /// lays out once.
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

    /// `drawParagraphWith` records the current layout using one fill paint.
    ///
    /// `paint` replaces every span's fill, while shadows and decorations retain
    /// their span styles. Later paragraph changes do not affect the display list.
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

    /// `backdropBlur` blurs existing target pixels beneath a local-space rectangle.
    ///
    /// `sigma` is the Gaussian standard deviation in local units. The active
    /// clip shapes the result; later draws appear above it.
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

    /// `build` consumes the builder and returns its immutable display list.
    ///
    /// Any unmatched save scopes are closed before the list is finalized.
    /// Calling `build` or any recording method again throws.
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
