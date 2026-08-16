use std::cell::RefCell;
use std::rc::Rc;

use valo::{
    Color, Context, Image, ImageDesc, MemoryReport, PersistentCanvas, RenderStats, Surface,
};
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

/// One GPU device, shared by every canvas attached to it.
///
/// This is the whole reason valo is worth putting on a page full of live
/// demos. A traditional 2D context carries its own device, and browsers cap
/// those at around 16 — so a dozen animated cards is close to the ceiling
/// before anything is drawn. Here the expensive things live on the device:
/// [`Context`] owns the glyph atlas, the image cache, the contour cache and
/// the render-target pool, and twelve canvases share ONE of each.
///
/// The device is refcounted rather than owned by a canvas, so canvases come
/// and go — a card scrolled out of the DOM frees its surface and its backing
/// — while the shared caches stay warm for the ones still drawing.
#[wasm_bindgen(js_name = Device)]
pub struct WebDevice {
    /// `RefCell` rather than a lock: wasm is single-threaded, and every
    /// borrow here is a straight-line render or upload that cannot re-enter.
    context: Rc<RefCell<Context>>,
    /// Retained because every `attach` needs them to build a swapchain.
    instance: wgpu::Instance,
    adapter: wgpu::Adapter,
    unrestricted_external_copies: bool,
}

/// One canvas on a [`WebDevice`]. Owns only what is genuinely per-canvas: the
/// swapchain and the persistent backing its pixels live in.
/// What one device holds, summed across every canvas on it.
///
/// The shared-device claim is a memory claim, so it needs a number a page can
/// actually print. `atlasBytes` is the one that matters most: the glyph atlas
/// is per-device, so twelve canvases drawing the same typeface pay for it
/// once.
#[wasm_bindgen(js_name = MemoryReport)]
pub struct WebMemoryReport {
    inner: MemoryReport,
}

#[wasm_bindgen(js_class = MemoryReport)]
impl WebMemoryReport {
    /// Everything valo accounts for itself, in bytes.
    #[wasm_bindgen(getter, js_name = totalBytes)]
    pub fn total_bytes(&self) -> u64 {
        self.inner.total_bytes()
    }

    /// Glyph atlas pages across both families — shared by every canvas.
    #[wasm_bindgen(getter, js_name = atlasBytes)]
    pub fn atlas_bytes(&self) -> u64 {
        self.inner.atlas.iter().map(|family| family.bytes).sum()
    }

    /// Uploaded images, deduped across canvases.
    #[wasm_bindgen(getter, js_name = imageBytes)]
    pub fn image_bytes(&self) -> u64 {
        self.inner.images.bytes
    }

    /// Pooled render targets: layer, snapshot and filter scratch, shared.
    #[wasm_bindgen(getter, js_name = targetBytes)]
    pub fn target_bytes(&self) -> u64 {
        self.inner.targets.bytes
    }

    #[wasm_bindgen(getter, js_name = targetCount)]
    pub fn target_count(&self) -> u32 {
        self.inner.targets.count
    }
}

#[wasm_bindgen(js_name = Renderer)]
pub struct WebRenderer {
    context: Rc<RefCell<Context>>,
    surface: Surface,
    /// Where the canvas's pixels actually live. A swapchain image is gone the
    /// moment it is presented, so Canvas2D's "what you drew stays drawn"
    /// needs storage of its own.
    canvas: PersistentCanvas,
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

/// Acquire one GPU device. Attach as many canvases to it as the page has.
#[wasm_bindgen(js_name = createDevice)]
pub async fn create_device() -> Result<WebDevice, JsValue> {
    console_error_panic_hook::set_once();
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
    let unrestricted_external_copies = adapter
        .get_downlevel_capabilities()
        .flags
        .contains(wgpu::DownlevelFlags::UNRESTRICTED_EXTERNAL_TEXTURE_COPIES);
    Ok(WebDevice {
        context: Rc::new(RefCell::new(Context::new(device, queue))),
        instance,
        adapter,
        unrestricted_external_copies,
    })
}

/// One device, one canvas — the single-canvas shorthand.
///
/// A page with several live canvases should call [`create_device`] once and
/// [`WebDevice::attach`] per canvas instead; this exists for the common case
/// of exactly one.
#[wasm_bindgen(js_name = createRenderer)]
pub async fn create_renderer(canvas: web_sys::HtmlCanvasElement) -> Result<WebRenderer, JsValue> {
    create_device().await?.attach(canvas)
}

#[wasm_bindgen(js_class = Device)]
impl WebDevice {
    /// Give `canvas` a renderer on this device.
    ///
    /// Only the swapchain and the persistent backing are allocated here — the
    /// atlases, caches and pools stay shared, which is the point.
    pub fn attach(&self, canvas: web_sys::HtmlCanvasElement) -> Result<WebRenderer, JsValue> {
        let size = [canvas.width().max(1), canvas.height().max(1)];
        let surface = Surface::new(
            &self.instance,
            &self.adapter,
            self.context.borrow().device(),
            wgpu::SurfaceTarget::Canvas(canvas),
            size,
        )
        .map_err(|error| JsValue::from_str(&format!("cannot create canvas surface: {error:?}")))?;
        let backing = PersistentCanvas::new(&mut self.context.borrow_mut(), size, surface.format());
        Ok(WebRenderer {
            context: Rc::clone(&self.context),
            canvas: backing,
            surface,
            unrestricted_external_copies: self.unrestricted_external_copies,
        })
    }

    /// How many canvases are still attached, this one aside.
    ///
    /// The device outlives its canvases: dropping a renderer releases only
    /// that canvas's swapchain and backing, and the shared caches survive for
    /// whoever is still drawing.
    #[wasm_bindgen(getter, js_name = attachedCanvases)]
    pub fn attached_canvases(&self) -> usize {
        // One reference is the device's own.
        Rc::strong_count(&self.context) - 1
    }

    /// What this device holds across EVERY canvas on it — the number the
    /// shared-device claim rests on.
    #[wasm_bindgen(js_name = memoryReport)]
    pub fn memory_report(&self) -> WebMemoryReport {
        WebMemoryReport {
            inner: self.context.borrow().memory_report(),
        }
    }
}

#[wasm_bindgen(js_class = Renderer)]
impl WebRenderer {
    /// Resize the swapchain and the canvas backing together. The backing's
    /// contents are dropped: Canvas2D specifies that setting `width` or
    /// `height` clears the canvas, so there is nothing to carry over.
    pub fn resize(&mut self, width: u32, height: u32) {
        self.surface.resize([width, height]);
        self.canvas
            .resize(&mut self.context.borrow_mut(), [width, height]);
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
    /// Draw one frame's DELTA onto the persistent canvas, then show it.
    ///
    /// `discard` throws away what was on the canvas and starts from the given
    /// colour — `reset`, `beginFrame`, or a full-surface `clearRect`. Without
    /// it the previous pixels are restored first, which is what makes N
    /// incremental frames cost O(N) instead of the O(N²) that replaying every
    /// past display list did.
    pub fn render(
        &mut self,
        list: &WebDisplayList,
        discard: bool,
        red: f32,
        green: f32,
        blue: f32,
        alpha: f32,
    ) -> Option<WebRenderStats> {
        let clear = discard.then_some(Color::rgba(red, green, blue, alpha));
        let inner = self
            .canvas
            .draw(&mut self.context.borrow_mut(), &list.inner, clear);

        // Presenting is a COPY, and an unavoidable one: WebGPU cannot hand an
        // arbitrary texture to the compositor the way Chrome hands over a
        // SharedImage, so the canvas has to be blitted into the swapchain
        // image. Nothing to optimise away here.
        //
        // A failed acquisition costs only this frame's presentation. The
        // pixels are already safe in the backing, so the next present shows
        // them.
        if let Some(frame) = self.surface.acquire() {
            self.canvas
                .present_to(&mut self.context.borrow_mut(), &frame.target(None));
            self.context.borrow().present(frame);
        }
        Some(WebRenderStats { inner })
    }

    #[wasm_bindgen(js_name = uploadImageBitmap)]
    pub fn upload_image_bitmap(
        &mut self,
        bitmap: &web_sys::ImageBitmap,
        mipmaps: bool,
    ) -> WebImage {
        WebImage {
            inner: self
                .context
                .borrow_mut()
                .upload_image_bitmap(bitmap, mipmaps),
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
            inner: self.context.borrow_mut().upload_image(
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
            inner: self.context.borrow_mut().upload_external_image(
                source,
                [width, height],
                mipmaps,
            ),
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
            inner: self.context.borrow_mut().upload_external_image_region(
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
            .borrow_mut()
            .refresh_external_image(&image.inner, source, [width, height]))
    }

    #[wasm_bindgen(js_name = setGestureHold)]
    pub fn set_gesture_hold(&mut self, held: bool) {
        let mut context = self.context.borrow_mut();
        context.set_text_raster_hold(held);
        context.set_raster_hold(held);
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
