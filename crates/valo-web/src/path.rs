use std::sync::Arc;

use valo::{Path, PathBuilder, Point, Rect};
use wasm_bindgen::prelude::*;

use crate::types;

#[wasm_bindgen(js_name = Path)]
pub struct WebPath {
    builder: PathBuilder,
    cached: Option<Arc<Path>>,
}

impl WebPath {
    pub(crate) fn built(&mut self) -> Arc<Path> {
        self.cached
            .get_or_insert_with(|| self.builder.clone().build())
            .clone()
    }

    fn change(&mut self, operation: impl FnOnce(&mut PathBuilder)) {
        operation(&mut self.builder);
        self.cached = None;
    }
}

#[wasm_bindgen(js_class = Path)]
impl WebPath {
    #[wasm_bindgen(constructor)]
    pub fn new() -> Self {
        Self {
            builder: PathBuilder::new(),
            cached: None,
        }
    }

    /// A retained snapshot for Canvas state such as clipping. Subsequent
    /// edits to either path do not affect the other.
    #[wasm_bindgen(js_name = clone)]
    pub fn duplicate(&self) -> Self {
        Self {
            builder: self.builder.clone(),
            cached: self.cached.clone(),
        }
    }

    #[wasm_bindgen(js_name = moveTo)]
    pub fn move_to(&mut self, x: f32, y: f32) {
        self.change(|path| {
            path.move_to((x, y));
        });
    }

    #[wasm_bindgen(js_name = lineTo)]
    pub fn line_to(&mut self, x: f32, y: f32) {
        self.change(|path| {
            path.line_to((x, y));
        });
    }

    #[wasm_bindgen(js_name = quadraticCurveTo)]
    pub fn quadratic_curve_to(&mut self, control_x: f32, control_y: f32, x: f32, y: f32) {
        self.change(|path| {
            path.quad_to((control_x, control_y), (x, y));
        });
    }

    #[wasm_bindgen(js_name = bezierCurveTo)]
    pub fn bezier_curve_to(
        &mut self,
        control1_x: f32,
        control1_y: f32,
        control2_x: f32,
        control2_y: f32,
        x: f32,
        y: f32,
    ) {
        self.change(|path| {
            path.cubic_to((control1_x, control1_y), (control2_x, control2_y), (x, y));
        });
    }

    pub fn close(&mut self) {
        self.change(|path| {
            path.close();
        });
    }

    pub fn rect(&mut self, x: f32, y: f32, width: f32, height: f32) {
        self.change(|path| {
            path.rect(Rect::new(x, y, width, height));
        });
    }

    #[wasm_bindgen(js_name = roundRect)]
    pub fn round_rect(
        &mut self,
        x: f32,
        y: f32,
        width: f32,
        height: f32,
        radii: &[f32],
    ) -> Result<(), JsValue> {
        let radii = elliptical_radii(radii)?;
        self.change(|path| {
            path.rrect_radii_elliptical(Rect::new(x, y, width, height), radii);
        });
        Ok(())
    }

    pub fn arc(
        &mut self,
        center_x: f32,
        center_y: f32,
        radius: f32,
        start_angle: f32,
        sweep_angle: f32,
    ) {
        self.change(|path| {
            path.arc((center_x, center_y), radius, start_angle, sweep_angle);
        });
    }

    #[allow(clippy::too_many_arguments)]
    pub fn ellipse(
        &mut self,
        center_x: f32,
        center_y: f32,
        radius_x: f32,
        radius_y: f32,
        rotation: f32,
        start_angle: f32,
        sweep_angle: f32,
    ) {
        self.change(|path| {
            path.ellipse(
                (center_x, center_y),
                [radius_x, radius_y],
                rotation,
                start_angle,
                sweep_angle,
            );
        });
    }

    #[wasm_bindgen(js_name = arcTo)]
    pub fn arc_to(&mut self, x1: f32, y1: f32, x2: f32, y2: f32, radius: f32) {
        self.change(|path| {
            path.arc_to((x1, y1), (x2, y2), radius);
        });
    }

    pub fn contains(&mut self, x: f32, y: f32, fill_rule: u32) -> bool {
        self.built()
            .contains(Point::new(x, y), types::fill_rule(fill_rule))
    }
}

impl Default for WebPath {
    fn default() -> Self {
        Self::new()
    }
}

pub(crate) fn elliptical_radii(values: &[f32]) -> Result<[[f32; 2]; 4], JsValue> {
    match values {
        [radius] => Ok([[*radius; 2]; 4]),
        [top_left, top_right, bottom_right, bottom_left] => Ok([
            [*top_left; 2],
            [*top_right; 2],
            [*bottom_right; 2],
            [*bottom_left; 2],
        ]),
        [tlx, tly, trx, try_, brx, bry, blx, bly] => {
            Ok([[*tlx, *tly], [*trx, *try_], [*brx, *bry], [*blx, *bly]])
        }
        _ => Err(JsValue::from_str("radii need 1, 4, or 8 values")),
    }
}
