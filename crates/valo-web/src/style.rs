use valo::{
    Color, ColorFilter, Dash, FocalCircle, GradientStop, ImageFilter, MaskBlur, Matrix, Paint,
    PaintStyle, Point, Shader, Stroke,
};
use wasm_bindgen::prelude::*;

use crate::renderer::WebImage;
use crate::types;

/// `WebColorFilter` transforms the pixels produced by a draw or layer.
///
/// Color filters run before mask blur, so the blur spreads the filtered result.
#[wasm_bindgen(js_name = ColorFilter)]
pub struct WebColorFilter {
    pub(crate) inner: ColorFilter,
}

#[wasm_bindgen(js_class = ColorFilter)]
impl WebColorFilter {
    /// `matrix` creates a row-major 4×5 transform over straight color in `0..=1`.
    ///
    /// `values` must contain exactly 20 numbers. Each output channel is
    /// `row · [r, g, b, a, 1]`, then clamped. Flutter's `ColorFilter.matrix`
    /// hands the translation column in unnormalized `0..=255` space, so entries
    /// 4, 9, 14, and 19 must be divided by 255 before they arrive here.
    #[wasm_bindgen(js_name = matrix)]
    pub fn matrix(values: &[f32]) -> Result<WebColorFilter, JsValue> {
        let values: [f32; 20] = values
            .try_into()
            .map_err(|_| JsValue::from_str("a color matrix needs exactly 20 values"))?;
        Ok(WebColorFilter {
            inner: ColorFilter::Matrix(values),
        })
    }

    /// `blend` composites a constant source color over each produced pixel.
    ///
    /// Color components are straight-alpha sRGB. `mode` is `0` Clear through
    /// `28` Luminosity (`3` is SrcOver). Values outside `0..=28` use SrcOver.
    #[wasm_bindgen(js_name = blend)]
    pub fn blend(red: f32, green: f32, blue: f32, alpha: f32, mode: u32) -> WebColorFilter {
        WebColorFilter {
            inner: ColorFilter::Blend(
                Color::rgba(red, green, blue, alpha),
                types::blend_mode(mode),
            ),
        }
    }
}

/// `WebImageFilter` transforms a rasterized draw or layer.
///
/// In a composition, the inner filter runs first and feeds the outer filter.
#[wasm_bindgen(js_name = ImageFilter)]
pub struct WebImageFilter {
    pub(crate) inner: ImageFilter,
}

#[wasm_bindgen(js_class = ImageFilter)]
impl WebImageFilter {
    /// `clone` returns an independent copy of this filter.
    #[wasm_bindgen(js_name = clone)]
    pub fn clone_filter(&self) -> WebImageFilter {
        WebImageFilter {
            inner: self.inner.clone(),
        }
    }

    /// `blur` creates a Gaussian image filter with nonnegative sigmas.
    ///
    /// `sigmaX` and `sigmaY` are standard deviations in local x and y units.
    /// Negative values are clamped to zero.
    #[wasm_bindgen(js_name = blur)]
    pub fn blur(sigma_x: f32, sigma_y: f32) -> WebImageFilter {
        WebImageFilter {
            inner: ImageFilter::blur(sigma_x, sigma_y),
        }
    }

    /// `color` creates an image filter from a color filter.
    #[wasm_bindgen(js_name = color)]
    pub fn color(filter: &WebColorFilter) -> WebImageFilter {
        WebImageFilter {
            inner: ImageFilter::color(filter.inner),
        }
    }

    /// `dropShadow` creates a shadow that retains the original input.
    ///
    /// `offsetX` and `offsetY` move the shadow in local coordinates. Sigmas are
    /// nonnegative Gaussian standard deviations; negative values are clamped to
    /// zero. Color components are straight-alpha sRGB.
    #[wasm_bindgen(js_name = dropShadow)]
    pub fn drop_shadow(
        offset_x: f32,
        offset_y: f32,
        sigma_x: f32,
        sigma_y: f32,
        red: f32,
        green: f32,
        blue: f32,
        alpha: f32,
    ) -> WebImageFilter {
        WebImageFilter {
            inner: ImageFilter::drop_shadow(
                Point::new(offset_x, offset_y),
                sigma_x,
                sigma_y,
                Color::rgba(red, green, blue, alpha),
            ),
        }
    }

    /// `compose` applies `inner` first and `outer` second.
    #[wasm_bindgen(js_name = compose)]
    pub fn compose(outer: &WebImageFilter, inner: &WebImageFilter) -> WebImageFilter {
        WebImageFilter {
            inner: ImageFilter::compose(outer.inner.clone(), inner.inner.clone()),
        }
    }
}

/// `WebShader` determines the color painted at each point of a drawing operation.
///
/// Shader coordinates begin in the draw's local space, so shaders follow the
/// same transforms as their geometry unless [`Self::set_transform`] adds another.
#[wasm_bindgen(js_name = Shader)]
pub struct WebShader {
    pub(crate) inner: Shader,
}

#[wasm_bindgen(js_class = Shader)]
impl WebShader {
    /// `linearGradient` interpolates colors along the line from start to end.
    ///
    /// `offsets` are stop positions from zero to one, in nondecreasing order.
    /// `colors` is four straight-alpha RGBA values per offset. Empty offsets or
    /// a color length other than `offsets.length × 4` throws. `spread` is `0`
    /// pad, `1` repeat, or `2` reflect; any other value uses pad.
    #[wasm_bindgen(js_name = linearGradient)]
    pub fn linear_gradient(
        start_x: f32,
        start_y: f32,
        end_x: f32,
        end_y: f32,
        offsets: &[f32],
        colors: &[f32],
        spread: u32,
    ) -> Result<WebShader, JsValue> {
        Ok(WebShader {
            inner: Shader::Linear {
                start: Point::new(start_x, start_y),
                end: Point::new(end_x, end_y),
                stops: gradient_stops(offsets, colors)?,
                spread: types::spread_mode(spread),
                local: Matrix::IDENTITY,
            },
        })
    }

    /// `radialGradient` interpolates between a start circle and an end circle.
    ///
    /// `(startX, startY, startRadius)` is the focus circle; a zero radius is a
    /// focal point. `(endX, endY, endRadius)` is the end circle. Stop arrays
    /// follow [`Self::linear_gradient`]. `spread` is `0` pad, `1` repeat, or
    /// `2` reflect; any other value uses pad.
    #[allow(clippy::too_many_arguments)]
    #[wasm_bindgen(js_name = radialGradient)]
    pub fn radial_gradient(
        start_x: f32,
        start_y: f32,
        start_radius: f32,
        end_x: f32,
        end_y: f32,
        end_radius: f32,
        offsets: &[f32],
        colors: &[f32],
        spread: u32,
    ) -> Result<WebShader, JsValue> {
        Ok(WebShader {
            inner: Shader::Radial {
                center: Point::new(end_x, end_y),
                radius: end_radius,
                stops: gradient_stops(offsets, colors)?,
                spread: types::spread_mode(spread),
                focus: Some(FocalCircle {
                    center: Point::new(start_x, start_y),
                    radius: start_radius,
                }),
                local: Matrix::IDENTITY,
            },
        })
    }

    /// `sweepGradient` interpolates colors around one full clockwise turn.
    ///
    /// `startAngle` is the zero-offset angle in radians clockwise from +x.
    /// Stop arrays follow [`Self::linear_gradient`].
    #[wasm_bindgen(js_name = sweepGradient)]
    pub fn sweep_gradient(
        center_x: f32,
        center_y: f32,
        start_angle: f32,
        offsets: &[f32],
        colors: &[f32],
    ) -> Result<WebShader, JsValue> {
        Ok(WebShader {
            inner: Shader::Sweep {
                center: Point::new(center_x, center_y),
                start_angle,
                stops: gradient_stops(offsets, colors)?,
                local: Matrix::IDENTITY,
            },
        })
    }

    /// `imagePattern` samples an image across the drawn geometry.
    ///
    /// `filter` is `0` linear or `1` nearest; any other value uses linear.
    /// `mipmap` is `0` none, `1` nearest, or `2` linear; any other value uses
    /// linear. `tileX` and `tileY` are `0` clamp, `1` repeat, `2` mirror, or
    /// `3` decal; any other value uses clamp.
    #[wasm_bindgen(js_name = imagePattern)]
    pub fn image_pattern(
        image: &WebImage,
        filter: u32,
        mipmap: u32,
        tile_x: u32,
        tile_y: u32,
    ) -> WebShader {
        WebShader {
            inner: Shader::Image {
                image: image.inner.clone(),
                sampling: types::sampling(filter, mipmap, tile_x, tile_y),
                local: Matrix::IDENTITY,
            },
        }
    }

    /// `setTransform` transforms this shader independently of the drawn geometry.
    ///
    /// `values` must be 6 affine numbers `[a, b, c, d, tx, ty]` or 16
    /// column-major matrix values. Any other length throws. A paint stores a
    /// clone of the shader, so call this before [`WebPaint::set_shader`], or
    /// assign the shader to the paint again afterward.
    #[wasm_bindgen(js_name = setTransform)]
    pub fn set_transform(&mut self, values: &[f32]) -> Result<(), JsValue> {
        let matrix = types::matrix(values)?;
        match &mut self.inner {
            Shader::Linear { local, .. }
            | Shader::Radial { local, .. }
            | Shader::Sweep { local, .. }
            | Shader::Image { local, .. } => *local = matrix,
        }
        Ok(())
    }
}

/// `WebPaint` describes how a drawing operation produces and composites pixels.
///
/// The constructor creates a solid-color fill using SrcOver blending. Shader
/// and image draws use the paint's alpha and ignore its RGB channels.
#[wasm_bindgen(js_name = Paint)]
pub struct WebPaint {
    pub(crate) inner: Paint,
}

#[wasm_bindgen(js_class = Paint)]
impl WebPaint {
    /// `new` creates a solid-color fill paint.
    ///
    /// Color components are straight-alpha sRGB and are not clamped.
    #[wasm_bindgen(constructor)]
    pub fn new(red: f32, green: f32, blue: f32, alpha: f32) -> WebPaint {
        WebPaint {
            inner: Paint::from_color(Color::rgba(red, green, blue, alpha)),
        }
    }

    /// `setColor` replaces the solid-draw color and the alpha used by shaders.
    ///
    /// Shader and image draws ignore the RGB channels.
    #[wasm_bindgen(js_name = setColor)]
    pub fn set_color(&mut self, red: f32, green: f32, blue: f32, alpha: f32) {
        self.inner.color = Color::rgba(red, green, blue, alpha);
    }

    /// `setBlendMode` controls compositing with destination pixels.
    ///
    /// `mode` is `0` Clear through `28` Luminosity (`3` is SrcOver). Values
    /// outside `0..=28` use SrcOver. Advanced modes that read destination
    /// pixels may require an additional render-pass break.
    #[wasm_bindgen(js_name = setBlendMode)]
    pub fn set_blend_mode(&mut self, mode: u32) {
        self.inner.blend_mode = types::blend_mode(mode);
    }

    /// `setFill` covers the geometry's interior.
    #[wasm_bindgen(js_name = setFill)]
    pub fn set_fill(&mut self) {
        self.inner.style = PaintStyle::Fill;
    }

    /// `setStroke` draws the geometry's outline.
    ///
    /// `width` is the full stroke width in path coordinates; zero is a hairline
    /// one device pixel wide, and a negative width draws nothing. `cap` is `0`
    /// butt, `1` round, or `2` square; any other value uses butt. `join` is `0`
    /// miter, `1` round, or `2` bevel; any other value uses miter.
    /// `miterLimit` is the maximum miter length divided by half the stroke
    /// width. An empty `dash` is a solid stroke; otherwise intervals alternate
    /// painted and skipped lengths starting with painted, and `dashOffset` is
    /// the phase into that cycle.
    #[allow(clippy::too_many_arguments)]
    #[wasm_bindgen(js_name = setStroke)]
    pub fn set_stroke(
        &mut self,
        width: f32,
        cap: u32,
        join: u32,
        miter_limit: f32,
        dash: &[f32],
        dash_offset: f32,
    ) {
        self.inner.style = PaintStyle::Stroke(Stroke {
            width,
            cap: types::cap(cap),
            join: types::join(join),
            miter_limit,
            dash: (!dash.is_empty()).then(|| Dash {
                intervals: dash.to_vec(),
                phase: dash_offset,
            }),
        });
    }

    /// `setShader` replaces the solid color with a per-pixel source.
    ///
    /// The paint stores a clone; later [`WebShader::set_transform`] calls on
    /// `shader` do not affect this paint unless the shader is assigned again.
    #[wasm_bindgen(js_name = setShader)]
    pub fn set_shader(&mut self, shader: &WebShader) {
        self.inner.shader = Some(shader.inner.clone());
    }

    /// `clearShader` restores solid-color drawing.
    #[wasm_bindgen(js_name = clearShader)]
    pub fn clear_shader(&mut self) {
        self.inner.shader = None;
    }

    /// `setMaskBlur` softens the draw's coverage with a Gaussian blur.
    ///
    /// `sigma` is the standard deviation in local units and follows the draw's
    /// transform; negative values are clamped to zero. `style` is `0` normal,
    /// `1` solid, `2` inner, or `3` outer; any other value uses normal.
    #[wasm_bindgen(js_name = setMaskBlur)]
    pub fn set_mask_blur(&mut self, sigma: f32, style: u32) {
        self.inner.mask_blur = Some(match types::blur_style(style) {
            valo::BlurStyle::Normal => MaskBlur::new(sigma),
            valo::BlurStyle::Solid => MaskBlur::solid(sigma),
            valo::BlurStyle::Inner => MaskBlur::inner(sigma),
            valo::BlurStyle::Outer => MaskBlur::outer(sigma),
        });
    }

    /// `clearMaskBlur` removes coverage blur from this paint.
    #[wasm_bindgen(js_name = clearMaskBlur)]
    pub fn clear_mask_blur(&mut self) {
        self.inner.mask_blur = None;
    }

    /// `setColorFilter` transforms produced colors before mask blur.
    #[wasm_bindgen(js_name = setColorFilter)]
    pub fn set_color_filter(&mut self, filter: &WebColorFilter) {
        self.inner.color_filter = Some(filter.inner);
    }

    /// `clearColorFilter` removes the color filter from this paint.
    #[wasm_bindgen(js_name = clearColorFilter)]
    pub fn clear_color_filter(&mut self) {
        self.inner.color_filter = None;
    }

    /// `setImageFilter` transforms the rasterized draw or layer.
    ///
    /// The paint stores a clone of the filter.
    #[wasm_bindgen(js_name = setImageFilter)]
    pub fn set_image_filter(&mut self, filter: &WebImageFilter) {
        self.inner.image_filter = Some(filter.inner.clone());
    }

    /// `clearImageFilter` removes the image filter from this paint.
    #[wasm_bindgen(js_name = clearImageFilter)]
    pub fn clear_image_filter(&mut self) {
        self.inner.image_filter = None;
    }
}

fn gradient_stops(offsets: &[f32], colors: &[f32]) -> Result<Vec<GradientStop>, JsValue> {
    if offsets.is_empty() || colors.len() != offsets.len() * 4 {
        return Err(JsValue::from_str(
            "gradient colors need four RGBA values per offset",
        ));
    }
    Ok(offsets
        .iter()
        .zip(colors.chunks_exact(4))
        .map(|(&offset, color)| GradientStop {
            offset,
            color: Color::rgba(color[0], color[1], color[2], color[3]),
        })
        .collect())
}
