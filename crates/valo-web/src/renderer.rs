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
    unrestricted_external_copies: bool,
}

/// The DOM sources `copyExternalImageToTexture` accepts, resolved from an
/// untyped JS value.
fn external_image_source(source: &JsValue) -> Result<wgpu::ExternalImageSource, JsValue> {
    use wgpu::ExternalImageSource;
    if let Some(bitmap) = source.dyn_ref::<web_sys::ImageBitmap>() {
        return Ok(ExternalImageSource::ImageBitmap(bitmap.clone()));
    }
    if let Some(element) = source.dyn_ref::<web_sys::HtmlImageElement>() {
        return Ok(ExternalImageSource::HTMLImageElement(element.clone()));
    }
    if let Some(element) = source.dyn_ref::<web_sys::HtmlCanvasElement>() {
        return Ok(ExternalImageSource::HTMLCanvasElement(element.clone()));
    }
    if let Some(element) = source.dyn_ref::<web_sys::HtmlVideoElement>() {
        return Ok(ExternalImageSource::HTMLVideoElement(element.clone()));
    }
    if let Some(canvas) = source.dyn_ref::<web_sys::OffscreenCanvas>() {
        return Ok(ExternalImageSource::OffscreenCanvas(canvas.clone()));
    }
    if let Some(data) = source.dyn_ref::<web_sys::ImageData>() {
        return Ok(ExternalImageSource::ImageData(data.clone()));
    }
    if let Some(frame) = source.dyn_ref::<web_sys::VideoFrame>() {
        // `Clone::clone` explicitly: `VideoFrame` also has a JS `clone()`
        // that allocates a second frame and returns a `Result`. Only the
        // handle needs copying here.
        return Ok(ExternalImageSource::VideoFrame(Clone::clone(frame)));
    }
    Err(JsValue::from_str(
        "not a supported image source: expected HTMLImageElement, HTMLCanvasElement, \
         HTMLVideoElement, ImageBitmap, OffscreenCanvas, ImageData or VideoFrame",
    ))
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
        unrestricted_external_copies: adapter
            .get_downlevel_capabilities()
            .flags
            .contains(wgpu::DownlevelFlags::UNRESTRICTED_EXTERNAL_TEXTURE_COPIES),
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
        self.context.present(frame);
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

    /// Whether `OffscreenCanvas` is a legal copy source on this adapter.
    /// Without it the caller has to route through an `ImageBitmap`, so this
    /// is a capability question rather than an error to discover mid-frame.
    #[wasm_bindgen(getter, js_name = supportsOffscreenCanvasSource)]
    pub fn supports_offscreen_canvas_source(&self) -> bool {
        self.unrestricted_external_copies
    }

    /// A DOM image source copied straight into a texture. `width`/`height`
    /// are the source's PIXEL dimensions; the browser throws if they do not
    /// match, so the caller reads them from the source it passes.
    #[wasm_bindgen(js_name = uploadExternalImage)]
    pub fn upload_external_image(
        &mut self,
        source: &JsValue,
        width: u32,
        height: u32,
        mipmaps: bool,
    ) -> Result<WebImage, JsValue> {
        let source = self.checked_source(source)?;
        if width == 0 || height == 0 {
            return Err(JsValue::from_str("an image source needs a non-zero size"));
        }
        Ok(WebImage {
            inner: self
                .context
                .upload_external_image(source, [width, height], mipmaps),
        })
    }

    /// A rectangle of a DOM source, copied into an image of exactly that
    /// size. `putImageData` uses it so a dirty rectangle costs its own area
    /// rather than the whole `ImageData`.
    #[allow(clippy::too_many_arguments)]
    #[wasm_bindgen(js_name = uploadExternalImageRegion)]
    pub fn upload_external_image_region(
        &mut self,
        source: &JsValue,
        source_x: u32,
        source_y: u32,
        width: u32,
        height: u32,
    ) -> Result<WebImage, JsValue> {
        let source = self.checked_source(source)?;
        if width == 0 || height == 0 {
            return Err(JsValue::from_str("an image region needs a non-zero size"));
        }
        Ok(WebImage {
            inner: self.context.upload_external_image_region(
                source,
                [source_x, source_y],
                [width, height],
                false,
            ),
        })
    }

    /// Re-copy a source whose pixels changed into the SAME image, keeping
    /// its texture and cached bind groups. `false` = the size changed and
    /// the caller must upload again.
    #[wasm_bindgen(js_name = refreshExternalImage)]
    pub fn refresh_external_image(
        &mut self,
        image: &WebImage,
        source: &JsValue,
        width: u32,
        height: u32,
    ) -> Result<bool, JsValue> {
        let source = self.checked_source(source)?;
        Ok(self
            .context
            .refresh_external_image(&image.inner, source, [width, height]))
    }

    #[wasm_bindgen(js_name = setGestureHold)]
    pub fn set_gesture_hold(&mut self, held: bool) {
        self.context.set_text_raster_hold(held);
        self.context.set_raster_hold(held);
    }
}

impl WebRenderer {
    /// Reject an `OffscreenCanvas` up front on an adapter that cannot copy
    /// one, rather than letting the browser raise an opaque exception from
    /// inside the queue submission.
    fn checked_source(&self, source: &JsValue) -> Result<wgpu::ExternalImageSource, JsValue> {
        let source = external_image_source(source)?;
        if matches!(source, wgpu::ExternalImageSource::OffscreenCanvas(_))
            && !self.unrestricted_external_copies
        {
            return Err(JsValue::from_str(
                "this adapter cannot copy from an OffscreenCanvas \
                 (no UNRESTRICTED_EXTERNAL_TEXTURE_COPIES); transfer it to an ImageBitmap first",
            ));
        }
        Ok(source)
    }
}
