use valo::{
    Color, ColorFilter, Dash, FocalCircle, GradientStop, ImageFilter, MaskBlur, Matrix, Paint,
    PaintStyle, Point, Shader, Stroke,
};
use wasm_bindgen::prelude::*;

use crate::renderer::WebImage;
use crate::types;

#[wasm_bindgen(js_name = ColorFilter)]
pub struct WebColorFilter {
    pub(crate) inner: ColorFilter,
}

#[wasm_bindgen(js_class = ColorFilter)]
impl WebColorFilter {
    #[wasm_bindgen(js_name = matrix)]
    pub fn matrix(values: &[f32]) -> Result<WebColorFilter, JsValue> {
        let values: [f32; 20] = values
            .try_into()
            .map_err(|_| JsValue::from_str("a color matrix needs exactly 20 values"))?;
        Ok(WebColorFilter {
            inner: ColorFilter::Matrix(values),
        })
    }

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

#[wasm_bindgen(js_name = ImageFilter)]
pub struct WebImageFilter {
    pub(crate) inner: ImageFilter,
}

#[wasm_bindgen(js_class = ImageFilter)]
impl WebImageFilter {
    #[wasm_bindgen(js_name = clone)]
    pub fn clone_filter(&self) -> WebImageFilter {
        WebImageFilter {
            inner: self.inner.clone(),
        }
    }

    #[wasm_bindgen(js_name = blur)]
    pub fn blur(sigma_x: f32, sigma_y: f32) -> WebImageFilter {
        WebImageFilter {
            inner: ImageFilter::blur(sigma_x, sigma_y),
        }
    }

    #[wasm_bindgen(js_name = color)]
    pub fn color(filter: &WebColorFilter) -> WebImageFilter {
        WebImageFilter {
            inner: ImageFilter::color(filter.inner),
        }
    }

    #[wasm_bindgen(js_name = compose)]
    pub fn compose(outer: &WebImageFilter, inner: &WebImageFilter) -> WebImageFilter {
        WebImageFilter {
            inner: ImageFilter::compose(outer.inner.clone(), inner.inner.clone()),
        }
    }
}

#[wasm_bindgen(js_name = Shader)]
pub struct WebShader {
    pub(crate) inner: Shader,
}

#[wasm_bindgen(js_class = Shader)]
impl WebShader {
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

    #[wasm_bindgen(js_name = imagePattern)]
    pub fn image_pattern(image: &WebImage, filter: u32, tile_x: u32, tile_y: u32) -> WebShader {
        WebShader {
            inner: Shader::Image {
                image: image.inner.clone(),
                sampling: types::sampling(filter, tile_x, tile_y),
                local: Matrix::IDENTITY,
            },
        }
    }

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

#[wasm_bindgen(js_name = Paint)]
pub struct WebPaint {
    pub(crate) inner: Paint,
}

#[wasm_bindgen(js_class = Paint)]
impl WebPaint {
    #[wasm_bindgen(constructor)]
    pub fn new(red: f32, green: f32, blue: f32, alpha: f32) -> WebPaint {
        WebPaint {
            inner: Paint::from_color(Color::rgba(red, green, blue, alpha)),
        }
    }

    #[wasm_bindgen(js_name = setColor)]
    pub fn set_color(&mut self, red: f32, green: f32, blue: f32, alpha: f32) {
        self.inner.color = Color::rgba(red, green, blue, alpha);
    }

    #[wasm_bindgen(js_name = setBlendMode)]
    pub fn set_blend_mode(&mut self, mode: u32) {
        self.inner.blend_mode = types::blend_mode(mode);
    }

    #[wasm_bindgen(js_name = setFill)]
    pub fn set_fill(&mut self) {
        self.inner.style = PaintStyle::Fill;
    }

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

    #[wasm_bindgen(js_name = setShader)]
    pub fn set_shader(&mut self, shader: &WebShader) {
        self.inner.shader = Some(shader.inner.clone());
    }

    #[wasm_bindgen(js_name = clearShader)]
    pub fn clear_shader(&mut self) {
        self.inner.shader = None;
    }

    #[wasm_bindgen(js_name = setMaskBlur)]
    pub fn set_mask_blur(&mut self, sigma: f32, style: u32) {
        self.inner.mask_blur = Some(match types::blur_style(style) {
            valo::BlurStyle::Normal => MaskBlur::new(sigma),
            valo::BlurStyle::Solid => MaskBlur::solid(sigma),
            valo::BlurStyle::Inner => MaskBlur::inner(sigma),
            valo::BlurStyle::Outer => MaskBlur::outer(sigma),
        });
    }

    #[wasm_bindgen(js_name = clearMaskBlur)]
    pub fn clear_mask_blur(&mut self) {
        self.inner.mask_blur = None;
    }

    #[wasm_bindgen(js_name = setColorFilter)]
    pub fn set_color_filter(&mut self, filter: &WebColorFilter) {
        self.inner.color_filter = Some(filter.inner);
    }

    #[wasm_bindgen(js_name = clearColorFilter)]
    pub fn clear_color_filter(&mut self) {
        self.inner.color_filter = None;
    }

    #[wasm_bindgen(js_name = setImageFilter)]
    pub fn set_image_filter(&mut self, filter: &WebImageFilter) {
        self.inner.image_filter = Some(filter.inner.clone());
    }

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
