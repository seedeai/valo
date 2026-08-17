use std::cell::RefCell;
use std::rc::Rc;

use valo::{
    Color, Context, Image, ImageDesc, MemoryReport, PersistentCanvas, RenderStats, Surface,
};
use wasm_bindgen::prelude::*;

use crate::recording::WebDisplayList;

/// `WebImage` is a handle to an immutable GPU image uploaded through a renderer.
///
/// Display lists retain the image; dropping every JavaScript handle releases the
/// texture after in-flight GPU use finishes. Dimensions are integer pixels.
#[wasm_bindgen(js_name = Image)]
pub struct WebImage {
    pub(crate) inner: Image,
}

#[wasm_bindgen(js_class = Image)]
impl WebImage {
    /// `width` is the image width in pixels.
    #[wasm_bindgen(getter)]
    pub fn width(&self) -> u32 {
        self.inner.size()[0]
    }

    /// `height` is the image height in pixels.
    #[wasm_bindgen(getter)]
    pub fn height(&self) -> u32 {
        self.inner.size()[1]
    }
}

/// `WebRenderStats` reports the work performed by one [`WebRenderer::render`] call.
#[wasm_bindgen(js_name = RenderStats)]
pub struct WebRenderStats {
    inner: RenderStats,
}

#[wasm_bindgen(js_class = RenderStats)]
impl WebRenderStats {
    /// `cpuMilliseconds` is CPU time spent rendering and submitting, in milliseconds.
    #[wasm_bindgen(getter, js_name = cpuMilliseconds)]
    pub fn cpu_milliseconds(&self) -> f32 {
        self.inner.cpu_ms
    }

    /// `gpuMilliseconds` is the previous resolved frame's GPU time in milliseconds.
    ///
    /// It is zero until a timestamp resolves, or when timestamps are unavailable.
    #[wasm_bindgen(getter, js_name = gpuMilliseconds)]
    pub fn gpu_milliseconds(&self) -> f32 {
        self.inner.gpu_ms
    }

    /// `draws` is the number of logical draws remaining after culling.
    #[wasm_bindgen(getter)]
    pub fn draws(&self) -> u32 {
        self.inner.draws
    }

    /// `drawCalls` is the number of encoded GPU draw commands.
    ///
    /// One logical draw may require several commands, while batched glyphs may
    /// share one.
    #[wasm_bindgen(getter, js_name = drawCalls)]
    pub fn draw_calls(&self) -> u32 {
        self.inner.draw_calls
    }

    /// `renderPasses` is the number of encoded render passes.
    #[wasm_bindgen(getter, js_name = renderPasses)]
    pub fn render_passes(&self) -> u32 {
        self.inner.render_passes
    }

    /// `filterPasses` is the number of encoded image-filter passes.
    #[wasm_bindgen(getter, js_name = filterPasses)]
    pub fn filter_passes(&self) -> u32 {
        self.inner.filter_passes
    }

    /// `culled` is the number of draws skipped because their bounds miss the target.
    #[wasm_bindgen(getter)]
    pub fn culled(&self) -> u32 {
        self.inner.culled
    }
}

/// `WebDevice` is one GPU device shared by every canvas attached to it.
///
/// This is the whole reason valo is worth putting on a page full of live
/// demos. A traditional 2D context carries its own device, and browsers cap
/// those at around 16 — so a dozen animated cards is close to the ceiling
/// before anything is drawn. Here the expensive things live on the device:
/// the glyph atlas, the image cache, the contour cache and the render-target
/// pool, and twelve canvases share one of each.
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

/// `WebMemoryReport` summarizes GPU resources retained by one [`WebDevice`].
///
/// Totals cover every canvas attached to that device. Byte counts are estimates
/// from resource descriptors. `atlasBytes` is the figure the shared-device
/// claim rests on: the glyph atlas is per-device, so twelve canvases drawing
/// the same typeface pay for it once.
#[wasm_bindgen(js_name = MemoryReport)]
pub struct WebMemoryReport {
    inner: MemoryReport,
}

#[wasm_bindgen(js_class = MemoryReport)]
impl WebMemoryReport {
    /// `totalBytes` is everything Valo accounts for itself, in bytes.
    ///
    /// Separate wgpu object counters are not included.
    #[wasm_bindgen(getter, js_name = totalBytes)]
    pub fn total_bytes(&self) -> u64 {
        self.inner.total_bytes()
    }

    /// `atlasBytes` is glyph-atlas GPU memory across mask, SDF, and color pages.
    ///
    /// Every canvas on the device shares this storage.
    #[wasm_bindgen(getter, js_name = atlasBytes)]
    pub fn atlas_bytes(&self) -> u64 {
        self.inner.atlas.iter().map(|family| family.bytes).sum()
    }

    /// `imageBytes` is uploaded image GPU memory, including mip levels.
    ///
    /// Images live on the device, so every attached canvas can draw the same handle.
    #[wasm_bindgen(getter, js_name = imageBytes)]
    pub fn image_bytes(&self) -> u64 {
        self.inner.images.bytes
    }

    /// `targetBytes` is pooled layer, snapshot, filter, and scratch target memory.
    #[wasm_bindgen(getter, js_name = targetBytes)]
    pub fn target_bytes(&self) -> u64 {
        self.inner.targets.bytes
    }

    /// `targetCount` is the number of live pooled render targets.
    #[wasm_bindgen(getter, js_name = targetCount)]
    pub fn target_count(&self) -> u32 {
        self.inner.targets.count
    }
}

/// `WebRenderer` is one canvas attached to a [`WebDevice`].
///
/// It owns only what is genuinely per-canvas: the swapchain and the persistent
/// backing its pixels live in. Atlases, image caches, and target pools stay on
/// the device.
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

/// `create_device` acquires one GPU device for the page.
///
/// Attach as many canvases to it as the page has. Throws if no WebGPU adapter
/// or device can be created.
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

/// `create_renderer` creates one device and attaches `canvas` to it.
///
/// A page with several live canvases should call [`create_device`] once and
/// [`WebDevice::attach`] per canvas instead; this exists for the common case
/// of exactly one. In the compat build it is also the WebGL2 entry point:
/// where WebGPU is missing this falls back to the GL backend, which is
/// canvas-first by nature and so only reachable from here.
#[wasm_bindgen(js_name = createRenderer)]
pub async fn create_renderer(canvas: web_sys::HtmlCanvasElement) -> Result<WebRenderer, JsValue> {
    match create_device().await {
        Ok(device) => device.attach(canvas),
        Err(error) => create_renderer_fallback(canvas, error).await,
    }
}

/// The WebGL2 path. On wgpu's GL backend the adapter can only be requested
/// with a `compatible_surface` — the GL context lives on the canvas — so the
/// order is inverted: surface first, then adapter, then device. Everything
/// created here is bound to this one canvas, which is why the compat story is
/// one renderer per canvas rather than one device for the page.
#[cfg(feature = "webgl")]
async fn create_renderer_fallback(
    canvas: web_sys::HtmlCanvasElement,
    _webgpu_error: JsValue,
) -> Result<WebRenderer, JsValue> {
    console_error_panic_hook::set_once();
    let instance = wgpu::Instance::default();
    let size = [canvas.width().max(1), canvas.height().max(1)];
    let raw_surface = instance
        .create_surface(wgpu::SurfaceTarget::Canvas(canvas))
        .map_err(|error| JsValue::from_str(&format!("cannot create canvas surface: {error:?}")))?;
    let adapter = instance
        .request_adapter(&wgpu::RequestAdapterOptions {
            power_preference: wgpu::PowerPreference::HighPerformance,
            compatible_surface: Some(&raw_surface),
            ..Default::default()
        })
        .await
        .map_err(|error| {
            JsValue::from_str(&format!(
                "neither WebGPU nor WebGL2 is available: {error:?}"
            ))
        })?;
    let (device, queue) = adapter
        .request_device(&wgpu::DeviceDescriptor {
            label: Some("valo.web.compat"),
            required_features: adapter.features() & wgpu::Features::TIMESTAMP_QUERY,
            // The default limits are WebGPU-sized and a WebGL2 device refuses
            // them; start from the WebGL2 floor and take what this adapter
            // actually offers.
            required_limits: wgpu::Limits::downlevel_webgl2_defaults()
                .using_resolution(adapter.limits()),
            ..Default::default()
        })
        .await
        .map_err(|error| JsValue::from_str(&format!("cannot create WebGL2 device: {error:?}")))?;
    let unrestricted_external_copies = adapter
        .get_downlevel_capabilities()
        .flags
        .contains(wgpu::DownlevelFlags::UNRESTRICTED_EXTERNAL_TEXTURE_COPIES);
    let surface = Surface::from_wgpu_surface(raw_surface, &adapter, &device, size);
    let context = Rc::new(RefCell::new(Context::new(device, queue)));
    let backing = PersistentCanvas::new(&mut context.borrow_mut(), size, surface.format());
    Ok(WebRenderer {
        context,
        canvas: backing,
        surface,
        unrestricted_external_copies,
    })
}

/// Without the `webgl` feature there is nothing to fall back to: report the
/// WebGPU failure as-is.
#[cfg(not(feature = "webgl"))]
async fn create_renderer_fallback(
    _canvas: web_sys::HtmlCanvasElement,
    webgpu_error: JsValue,
) -> Result<WebRenderer, JsValue> {
    Err(webgpu_error)
}

#[wasm_bindgen(js_class = Device)]
impl WebDevice {
    /// `attach` gives `canvas` a renderer on this device.
    ///
    /// Only the swapchain and the persistent backing are allocated here — the
    /// atlases, caches and pools stay shared, which is the point. A WebGL
    /// device throws if a second canvas is attached; create another device
    /// instead. Canvas width and height of zero are treated as one pixel.
    pub fn attach(&self, canvas: web_sys::HtmlCanvasElement) -> Result<WebRenderer, JsValue> {
        // WebGL is one context per canvas by design, and wgpu's GL backend
        // renders every surface into the last one created (gfx-rs/wgpu#2343).
        // Refusing here turns a silent black canvas into an explanation.
        if self.adapter.get_info().backend == wgpu::Backend::Gl && self.attached_canvases() >= 1 {
            return Err(JsValue::from_str(
                "this device is running on WebGL, which drives one canvas per device; \
                 create a second Device for a second canvas",
            ));
        }
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

    /// `attachedCanvases` is how many canvases are still attached to this device.
    ///
    /// The device outlives its canvases: dropping a renderer releases only
    /// that canvas's swapchain and backing, and the shared caches survive for
    /// whoever is still drawing.
    #[wasm_bindgen(getter, js_name = attachedCanvases)]
    pub fn attached_canvases(&self) -> usize {
        // One reference is the device's own.
        Rc::strong_count(&self.context) - 1
    }

    /// `memoryReport` returns resource counts and estimated GPU memory usage.
    ///
    /// The totals cover every canvas still attached to this device.
    #[wasm_bindgen(js_name = memoryReport)]
    pub fn memory_report(&self) -> WebMemoryReport {
        WebMemoryReport {
            inner: self.context.borrow().memory_report(),
        }
    }
}

#[wasm_bindgen(js_class = Renderer)]
impl WebRenderer {
    /// `resize` resizes the swapchain and the canvas backing together.
    ///
    /// The backing's contents are dropped: Canvas2D specifies that setting
    /// `width` or `height` clears the canvas, so there is nothing to carry
    /// over. A zero extent is treated as one pixel.
    pub fn resize(&mut self, width: u32, height: u32) {
        self.surface.resize([width, height]);
        self.canvas
            .resize(&mut self.context.borrow_mut(), [width, height]);
    }

    /// `width` is the canvas backing width in pixels.
    #[wasm_bindgen(getter)]
    pub fn width(&self) -> u32 {
        self.surface.size()[0]
    }

    /// `height` is the canvas backing height in pixels.
    #[wasm_bindgen(getter)]
    pub fn height(&self) -> u32 {
        self.surface.size()[1]
    }

    #[allow(clippy::too_many_arguments)]
    /// `render` draws one frame's delta onto the persistent canvas, then shows it.
    ///
    /// `discard` throws away what was on the canvas and starts from the given
    /// colour — `reset`, `beginFrame`, or a full-surface `clearRect`. Without
    /// it the previous pixels are restored first, which is what makes N
    /// incremental frames cost O(N) instead of the O(N²) that replaying every
    /// past display list did. Color components are straight-alpha sRGB.
    ///
    /// A failed swapchain acquire skips only this frame's presentation; the
    /// pixels remain in the backing. Stats are always returned.
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

    /// `uploadImageBitmap` copies a decoded `ImageBitmap` into a retained image.
    ///
    /// Pixels go directly to the GPU without passing through WebAssembly memory.
    /// Set `mipmaps` when the image will be drawn smaller than its source size.
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

    /// `uploadRgba` uploads RGBA8 pixels and returns a retained image.
    ///
    /// `pixels` must contain exactly `width × height × 4` bytes, and both
    /// extents must be nonzero. When `premultiplied` is false, Valo
    /// premultiplies RGB during upload. Set `mipmaps` when the image may be
    /// drawn smaller than its source size.
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

    /// `supportsOffscreenCanvasSource` reports whether `OffscreenCanvas` is a legal copy source.
    ///
    /// Without it the caller has to route through an `ImageBitmap`, so this
    /// is a capability question rather than an error to discover mid-frame.
    #[wasm_bindgen(getter, js_name = supportsOffscreenCanvasSource)]
    pub fn supports_offscreen_canvas_source(&self) -> bool {
        self.unrestricted_external_copies
    }

    /// `uploadExternalImage` copies a DOM image source straight into a texture.
    ///
    /// Supported sources are `HTMLImageElement`, `HTMLCanvasElement`,
    /// `HTMLVideoElement`, `ImageBitmap`, `OffscreenCanvas`, `ImageData`, and
    /// `VideoFrame`. `width` and `height` are the source's pixel dimensions;
    /// the browser throws if they do not match, so the caller reads them from
    /// the source it passes. Zero size throws. The source must be ready;
    /// unlike Canvas2D, an undecoded image is not silently ignored.
    /// `OffscreenCanvas` throws up front when [`Self::supports_offscreen_canvas_source`]
    /// is false.
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

    /// `uploadExternalImageRegion` copies a rectangle of a DOM source into a new image.
    ///
    /// `sourceX` and `sourceY` are the region's top-left in source pixels.
    /// `width` and `height` are both the copied region and the returned image
    /// size. Zero size throws. Mipmaps are not generated. `putImageData` uses
    /// this so a dirty rectangle costs its own area rather than the whole
    /// `ImageData`.
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

    /// `refreshExternalImage` re-copies a source whose pixels changed into the same image.
    ///
    /// Use it for changing canvas or video frames to keep the image handle and
    /// its cached bind groups. Returns `false` without copying when `width` or
    /// `height` differs from the existing image; the caller must upload again.
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

    /// `setGestureHold` lets existing rasters stand in while a zoom or pan gesture is active.
    ///
    /// Enable it when the gesture starts and clear it when the view settles so
    /// text and display-list caches refill at the final scale. Bitmap-mask and
    /// SDF text can reuse a resident size; vector outlines are unaffected.
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
