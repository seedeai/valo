use valo::{Color, Context, Image, ImageDesc, RenderStats, Surface};
use wasm_bindgen::prelude::*;

use crate::recording::WebDisplayList;

#[wasm_bindgen(js_name = Image)]
pub struct WebImage {
    pub(crate) inner: Image,
}

#[wasm_bindgen(js_class = Image)]
impl WebImage {
    #[wasm_bindgen(getter)]
    pub fn width(&self) -> u32 {
        self.inner.size()[0]
    }

    #[wasm_bindgen(getter)]
    pub fn height(&self) -> u32 {
        self.inner.size()[1]
    }
}

#[wasm_bindgen(js_name = RenderStats)]
pub struct WebRenderStats {
    inner: RenderStats,
}

#[wasm_bindgen(js_class = RenderStats)]
impl WebRenderStats {
    #[wasm_bindgen(getter, js_name = cpuMilliseconds)]
    pub fn cpu_milliseconds(&self) -> f32 {
        self.inner.cpu_ms
    }

    #[wasm_bindgen(getter, js_name = gpuMilliseconds)]
    pub fn gpu_milliseconds(&self) -> f32 {
        self.inner.gpu_ms
    }

    #[wasm_bindgen(getter)]
    pub fn draws(&self) -> u32 {
        self.inner.draws
    }

    #[wasm_bindgen(getter, js_name = drawCalls)]
    pub fn draw_calls(&self) -> u32 {
        self.inner.draw_calls
    }

    #[wasm_bindgen(getter, js_name = renderPasses)]
    pub fn render_passes(&self) -> u32 {
        self.inner.render_passes
    }

    #[wasm_bindgen(getter, js_name = filterPasses)]
    pub fn filter_passes(&self) -> u32 {
        self.inner.filter_passes
    }

    #[wasm_bindgen(getter)]
    pub fn culled(&self) -> u32 {
        self.inner.culled
    }
}

#[wasm_bindgen(js_name = Renderer)]
pub struct WebRenderer {
    context: Context,
    surface: Surface,
}

#[wasm_bindgen(js_name = createRenderer)]
pub async fn create_renderer(canvas: web_sys::HtmlCanvasElement) -> Result<WebRenderer, JsValue> {
    console_error_panic_hook::set_once();
    let size = [canvas.width().max(1), canvas.height().max(1)];
    let instance = wgpu::Instance::default();
    let adapter = instance
        .request_adapter(&wgpu::RequestAdapterOptions {
            power_preference: wgpu::PowerPreference::HighPerformance,
            ..Default::default()
        })
        .await
        .map_err(|error| JsValue::from_str(&format!("no WebGPU adapter: {error:?}")))?;
    let (device, queue) = adapter
        .request_device(&wgpu::DeviceDescriptor {
            label: Some("valo.web"),
            required_features: adapter.features() & wgpu::Features::TIMESTAMP_QUERY,
            ..Default::default()
        })
        .await
        .map_err(|error| JsValue::from_str(&format!("cannot create WebGPU device: {error:?}")))?;
    let surface = Surface::new(
        &instance,
        &adapter,
        &device,
        wgpu::SurfaceTarget::Canvas(canvas),
        size,
    )
    .map_err(|error| JsValue::from_str(&format!("cannot create canvas surface: {error:?}")))?;
    Ok(WebRenderer {
        context: Context::new(device, queue),
        surface,
    })
}

#[wasm_bindgen(js_class = Renderer)]
impl WebRenderer {
    pub fn resize(&mut self, width: u32, height: u32) {
        self.surface.resize([width, height]);
    }

    #[wasm_bindgen(getter)]
    pub fn width(&self) -> u32 {
        self.surface.size()[0]
    }

    #[wasm_bindgen(getter)]
    pub fn height(&self) -> u32 {
        self.surface.size()[1]
    }

    #[allow(clippy::too_many_arguments)]
    pub fn render(
        &mut self,
        list: &WebDisplayList,
        clear: bool,
        red: f32,
        green: f32,
        blue: f32,
        alpha: f32,
    ) -> Option<WebRenderStats> {
        let frame = self.surface.acquire()?;
        let clear = clear.then_some(Color::rgba(red, green, blue, alpha));
        let inner = self.context.render(&list.inner, &frame.target(clear));
        frame.present();
        Some(WebRenderStats { inner })
    }

    #[wasm_bindgen(js_name = uploadImageBitmap)]
    pub fn upload_image_bitmap(
        &mut self,
        bitmap: &web_sys::ImageBitmap,
        mipmaps: bool,
    ) -> WebImage {
        WebImage {
            inner: self.context.upload_image_bitmap(bitmap, mipmaps),
        }
    }

    #[allow(clippy::too_many_arguments)]
    #[wasm_bindgen(js_name = uploadRgba)]
    pub fn upload_rgba(
        &mut self,
        width: u32,
        height: u32,
        pixels: &[u8],
        premultiplied: bool,
        mipmaps: bool,
    ) -> Result<WebImage, JsValue> {
        let expected = usize::try_from(width)
            .ok()
            .zip(usize::try_from(height).ok())
            .filter(|&(width, height)| width > 0 && height > 0)
            .and_then(|(width, height)| width.checked_mul(height))
            .and_then(|pixels| pixels.checked_mul(4));
        if expected != Some(pixels.len()) {
            return Err(JsValue::from_str(
                "RGBA data length does not match its dimensions",
            ));
        }
        Ok(WebImage {
            inner: self.context.upload_image(
                ImageDesc {
                    size: [width, height],
                    premultiplied,
                    mips: mipmaps,
                },
                pixels,
            ),
        })
    }

    #[wasm_bindgen(js_name = setGestureHold)]
    pub fn set_gesture_hold(&mut self, held: bool) {
        self.context.set_text_raster_hold(held);
        self.context.set_raster_hold(held);
    }
}
