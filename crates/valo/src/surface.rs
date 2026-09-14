use valo_dl::{DisplayList, DisplayListBuilder, Image};
use valo_geometry::{Color, Rect};
use valo_renderer::{RenderStats, RenderTarget};

/// `SurfaceAlpha` says whether the compositor honours the surface's alpha channel.
///
/// Valo renders premultiplied alpha either way; this only decides whether the platform
/// looks at it. With `Opaque` the surface hides everything behind it, which is right for an
/// ordinary window. With `Transparent` the pixels a frame leaves clear show what is behind
/// the surface: use it for a window or canvas the host has made non-opaque, such as one
/// with a blur view behind it. Blending costs the compositor a pass per frame, so leave it
/// off otherwise.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum SurfaceAlpha {
    /// The compositor ignores alpha; the surface hides everything behind it.
    #[default]
    Opaque,
    /// The compositor honours alpha; clear pixels show what is behind the surface.
    Transparent,
}

/// `SurfaceOptions` selects how a surface is configured beyond its size.
///
/// `SurfaceOptions::default()` is what the plain constructors use. Non-exhaustive so later
/// choices can join without breaking callers: start from the default and set what differs,
/// as in `SurfaceOptions::default().with_alpha(SurfaceAlpha::Transparent)`.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
#[non_exhaustive]
pub struct SurfaceOptions {
    /// Whether the compositor honours the surface's alpha channel; `Opaque` by default.
    pub alpha: SurfaceAlpha,
}

impl SurfaceOptions {
    /// `with_alpha` returns these options asking for `alpha`.
    pub fn with_alpha(mut self, alpha: SurfaceAlpha) -> Self {
        self.alpha = alpha;
        self
    }
}

/// `Surface` manages a presentable native window or browser canvas.
///
/// Render each frame by calling `acquire`, [`crate::Context::render`], and
/// [`crate::Context::present`]. Valo selects a format that preserves its
/// CSS/Skia-compatible sRGB blending. Pass [`SurfaceOptions`] to the `_with_options`
/// constructors for a surface that shows what is behind it.
pub struct Surface {
    surface: wgpu::Surface<'static>,
    config: wgpu::SurfaceConfiguration,
    device: wgpu::Device,
    alpha: SurfaceAlpha,
}

impl Surface {
    /// `set_presents_with_transaction` synchronizes Metal presentation with Core Animation.
    ///
    /// Call on the main thread before acquiring a frame. Enable this for AppKit
    /// hosts that render inside resize callbacks, so the new drawable and window
    /// geometry are committed together. wgpu waits for GPU scheduling before
    /// presenting the drawable. Returns false for a non-Metal surface.
    #[cfg(target_os = "macos")]
    pub fn set_presents_with_transaction(&mut self, enabled: bool) -> bool {
        assert!(
            objc2::MainThreadMarker::new().is_some(),
            "presentation configuration requires the main thread"
        );
        // The guard keeps the surface alive; only the layer's presentation mode
        // changes, under its lock. No HAL resource is destroyed or replaced.
        let Some(surface) = (unsafe { self.surface.as_hal::<wgpu::hal::api::Metal>() }) else {
            return false;
        };
        surface
            .render_layer()
            .lock()
            .setPresentsWithTransaction(enabled);
        true
    }

    /// `new` creates and configures a surface over a window or canvas.
    pub fn new(
        instance: &wgpu::Instance,
        adapter: &wgpu::Adapter,
        device: &wgpu::Device,
        target: impl Into<wgpu::SurfaceTarget<'static>>,
        size: [u32; 2],
    ) -> Result<Self, wgpu::CreateSurfaceError> {
        Self::new_with_options(
            instance,
            adapter,
            device,
            target,
            size,
            SurfaceOptions::default(),
        )
    }

    /// `new_with_options` creates and configures a surface over a window or canvas as
    /// `options` asks.
    pub fn new_with_options(
        instance: &wgpu::Instance,
        adapter: &wgpu::Adapter,
        device: &wgpu::Device,
        target: impl Into<wgpu::SurfaceTarget<'static>>,
        size: [u32; 2],
        options: SurfaceOptions,
    ) -> Result<Self, wgpu::CreateSurfaceError> {
        let surface = instance.create_surface(target)?;
        Ok(Self::from_wgpu_surface_with_options(
            surface, adapter, device, size, options,
        ))
    }

    /// `new_unsafe` creates a surface from raw platform handles.
    ///
    /// Use it when the embedder owns handles such as a `CAMetalLayer*` or
    /// `HWND` instead of a window object.
    ///
    /// # Safety
    /// Every raw handle in `target` must remain valid for the surface's lifetime.
    pub unsafe fn new_unsafe(
        instance: &wgpu::Instance,
        adapter: &wgpu::Adapter,
        device: &wgpu::Device,
        target: wgpu::SurfaceTargetUnsafe,
        size: [u32; 2],
    ) -> Result<Self, wgpu::CreateSurfaceError> {
        unsafe {
            Self::new_unsafe_with_options(
                instance,
                adapter,
                device,
                target,
                size,
                SurfaceOptions::default(),
            )
        }
    }

    /// `new_unsafe_with_options` creates a surface from raw platform handles as `options`
    /// asks.
    ///
    /// # Safety
    /// Every raw handle in `target` must remain valid for the surface's lifetime.
    pub unsafe fn new_unsafe_with_options(
        instance: &wgpu::Instance,
        adapter: &wgpu::Adapter,
        device: &wgpu::Device,
        target: wgpu::SurfaceTargetUnsafe,
        size: [u32; 2],
        options: SurfaceOptions,
    ) -> Result<Self, wgpu::CreateSurfaceError> {
        let surface = unsafe { instance.create_surface_unsafe(target)? };
        Ok(Self::from_wgpu_surface_with_options(
            surface, adapter, device, size, options,
        ))
    }

    /// `from_wgpu_surface` configures an existing wgpu surface for Valo.
    ///
    /// This supports WebGL hosts that must create a canvas surface before
    /// requesting a compatible adapter.
    pub fn from_wgpu_surface(
        surface: wgpu::Surface<'static>,
        adapter: &wgpu::Adapter,
        device: &wgpu::Device,
        size: [u32; 2],
    ) -> Self {
        Self::from_wgpu_surface_with_options(
            surface,
            adapter,
            device,
            size,
            SurfaceOptions::default(),
        )
    }

    /// `from_wgpu_surface_with_options` configures an existing wgpu surface for Valo as
    /// `options` asks.
    ///
    /// A backend that cannot honour the alpha asked for gets the nearest it offers;
    /// [`alpha`](Self::alpha) says what was in effect.
    pub fn from_wgpu_surface_with_options(
        surface: wgpu::Surface<'static>,
        adapter: &wgpu::Adapter,
        device: &wgpu::Device,
        size: [u32; 2],
        options: SurfaceOptions,
    ) -> Self {
        let caps = surface.get_capabilities(adapter);
        let alpha_mode =
            alpha_mode_for(options.alpha, adapter.get_info().backend, &caps.alpha_modes);
        let format = caps
            .formats
            .iter()
            .copied()
            .find(|f| !f.is_srgb())
            .unwrap_or(caps.formats[0]);
        let config = wgpu::SurfaceConfiguration {
            // Blending happens in sRGB space, and the format picked above is
            // non-sRGB to keep it there. Linear-light blending would be
            // physically "more correct" but diverge from every browser and
            // from Skia — Canvas2D parity is the goal, so sRGB it is.
            color_space: wgpu::SurfaceColorSpace::Srgb,
            // COPY_SRC where the platform allows it: advanced blends snapshot
            // the resolved target mid-frame when rendering direct to the
            // swapchain. WebGL2's default framebuffer cannot be a copy source
            // — and never needs to be, because on that path every frame blits
            // from the persistent backing, which carries its own COPY_SRC.
            usage: (wgpu::TextureUsages::RENDER_ATTACHMENT | wgpu::TextureUsages::COPY_SRC)
                & caps.usages,
            format,
            width: size[0].max(1),
            height: size[1].max(1),
            present_mode: wgpu::PresentMode::AutoVsync,
            desired_maximum_frame_latency: 2,
            alpha_mode,
            view_formats: vec![],
        };
        surface.configure(device, &config);
        Self {
            surface,
            config,
            device: device.clone(),
            alpha: alpha_in_effect(alpha_mode),
        }
    }

    /// `alpha` returns the alpha treatment in effect, which is what was asked for unless
    /// the backend offers no way to honour it.
    pub fn alpha(&self) -> SurfaceAlpha {
        self.alpha
    }

    /// `resize` reconfigures the surface after its window or canvas changes size.
    pub fn resize(&mut self, size: [u32; 2]) {
        self.config.width = size[0].max(1);
        self.config.height = size[1].max(1);
        self.surface.configure(&self.device, &self.config);
    }

    /// `size` returns the configured surface dimensions in pixels.
    pub fn size(&self) -> [u32; 2] {
        [self.config.width, self.config.height]
    }

    /// `format` returns the selected surface format.
    pub fn format(&self) -> wgpu::TextureFormat {
        self.config.format
    }

    /// `acquire` returns the next frame, or why there is none this time.
    ///
    /// Lost or outdated surfaces are reconfigured and retried once; the other
    /// refusals are the caller's to answer, each differently (see [`Refused`]).
    pub fn acquire(&mut self) -> Result<SurfaceFrame, Refused> {
        use wgpu::CurrentSurfaceTexture as C;
        for _ in 0..2 {
            match self.surface.get_current_texture() {
                C::Success(t) | C::Suboptimal(t) => {
                    let raw = t.texture.clone();
                    let view = raw.create_view(&wgpu::TextureViewDescriptor::default());
                    return Ok(SurfaceFrame {
                        surface_texture: t,
                        raw,
                        view,
                        format: self.config.format,
                        size: [self.config.width, self.config.height],
                    });
                }
                C::Occluded => return Err(Refused::Occluded),
                C::Timeout => return Err(Refused::Timeout),
                C::Outdated | C::Lost => self.surface.configure(&self.device, &self.config),
                _ => return Err(Refused::Lost),
            }
        }
        Err(Refused::Lost)
    }
}

/// `Refused` is why [`Surface::acquire`] gave no frame, which decides what the
/// caller does next.
///
/// A host that keeps the frame it could not draw answers each kind on its own
/// terms: it waits for the system's word on an occluded window, tries again at
/// the next refresh after a timeout, and gives up on a lost surface.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Refused {
    /// The window is not on screen as the system reports it. On macOS wgpu asks
    /// AppKit's occlusion state before asking the layer for a drawable, and a
    /// window just ordered front stays occluded until AppKit says otherwise;
    /// that word (`NSWindowDidChangeOcclusionState`) is when a frame will take.
    Occluded,
    /// The layer gave no drawable within its time: the display holds every one.
    /// The next refresh frees one.
    Timeout,
    /// The surface could not be brought back after a reconfiguration, or the
    /// device is out of memory: nothing takes a frame until the surface is
    /// made again.
    Lost,
}

/// The wgpu mode that gives `alpha` on this backend, from the modes the surface offers.
///
/// Valo's pixels are premultiplied, so `PreMultiplied` is the mode where offered. Metal
/// offers only `PostMultiplied`, which there does nothing but clear the layer's opaque
/// flag, after which Core Animation composites premultiplied, so it is taken on Metal
/// alone. WebGPU accepts `PreMultiplied` though its capabilities list `Opaque` only.
fn alpha_mode_for(
    alpha: SurfaceAlpha,
    backend: wgpu::Backend,
    offered: &[wgpu::CompositeAlphaMode],
) -> wgpu::CompositeAlphaMode {
    use wgpu::CompositeAlphaMode as Mode;
    let first = offered.first().copied().unwrap_or(Mode::Auto);
    if alpha == SurfaceAlpha::Opaque {
        return first;
    }
    if backend == wgpu::Backend::BrowserWebGpu {
        return Mode::PreMultiplied;
    }
    if offered.contains(&Mode::PreMultiplied) {
        return Mode::PreMultiplied;
    }
    if backend == wgpu::Backend::Metal && offered.contains(&Mode::PostMultiplied) {
        return Mode::PostMultiplied;
    }
    if offered.contains(&Mode::Inherit) {
        return Mode::Inherit;
    }
    first
}

/// What a configured mode amounts to for the caller.
fn alpha_in_effect(mode: wgpu::CompositeAlphaMode) -> SurfaceAlpha {
    match mode {
        wgpu::CompositeAlphaMode::Opaque | wgpu::CompositeAlphaMode::Auto => SurfaceAlpha::Opaque,
        _ => SurfaceAlpha::Transparent,
    }
}

/// `SurfaceFrame` is one acquired surface frame ready for rendering.
pub struct SurfaceFrame {
    surface_texture: wgpu::SurfaceTexture,
    raw: wgpu::Texture,
    view: wgpu::TextureView,
    /// `format` is this frame's pixel format.
    pub format: wgpu::TextureFormat,
    /// `size` is this frame's dimensions in pixels.
    pub size: [u32; 2],
}

impl SurfaceFrame {
    /// `target` creates a render target over this frame.
    ///
    /// Pass `None` to preserve the frame's existing pixels.
    pub fn target(&self, clear: Option<Color>) -> RenderTarget<'_> {
        RenderTarget {
            view: &self.view,
            texture: &self.raw,
            format: self.format,
            size: self.size,
            clear,
        }
    }

    /// `present` hands the frame to the compositor and consumes it.
    ///
    /// Use the queue that submitted this frame's rendering commands.
    pub fn present(self, queue: &wgpu::Queue) {
        queue.present(self.surface_texture);
    }
}

/// `Offscreen` is a copyable render target that does not require a display.
///
/// Use it for headless rendering, snapshots, and image export.
pub struct Offscreen {
    texture: wgpu::Texture,
    view: wgpu::TextureView,
    /// `format` is always [`Offscreen::FORMAT`].
    pub format: wgpu::TextureFormat,
    /// `size` is the target's dimensions in pixels.
    pub size: [u32; 2],
}

impl Offscreen {
    /// `FORMAT` is the RGBA8 format used by every offscreen target.
    pub const FORMAT: wgpu::TextureFormat = wgpu::TextureFormat::Rgba8Unorm;

    /// `new` allocates a renderable and copyable offscreen target.
    pub fn new(device: &wgpu::Device, size: [u32; 2]) -> Self {
        let texture = device.create_texture(&wgpu::TextureDescriptor {
            label: Some("valo.offscreen"),
            size: wgpu::Extent3d {
                width: size[0],
                height: size[1],
                depth_or_array_layers: 1,
            },
            mip_level_count: 1,
            sample_count: 1,
            dimension: wgpu::TextureDimension::D2,
            format: Self::FORMAT,
            usage: wgpu::TextureUsages::RENDER_ATTACHMENT | wgpu::TextureUsages::COPY_SRC,
            view_formats: &[],
        });
        let view = texture.create_view(&wgpu::TextureViewDescriptor::default());
        Self {
            texture,
            view,
            format: Self::FORMAT,
            size,
        }
    }

    /// `target` creates a render target over the offscreen texture.
    pub fn target(&self, clear: Option<Color>) -> RenderTarget<'_> {
        RenderTarget {
            view: &self.view,
            texture: &self.texture,
            format: self.format,
            size: self.size,
            clear,
        }
    }

    /// `texture` returns the underlying texture for readback or further GPU work.
    pub fn texture(&self) -> &wgpu::Texture {
        &self.texture
    }
}

/// `PersistentCanvas` retains pixels across incremental frames.
///
/// Unlike a swapchain, it preserves previous pixels while applying new display
/// lists. This avoids replaying the full drawing history in paint, annotation,
/// and other incremental applications.
pub struct PersistentCanvas {
    // Restoring prior pixels samples one texture while rendering into the
    // other; WebGPU does not allow both roles on one texture in the same pass.
    front: Image,
    back: Image,
    size: [u32; 2],
    format: wgpu::TextureFormat,
    /// Nothing has been drawn yet, so there is nothing to restore.
    painted: bool,
}

impl PersistentCanvas {
    /// `new` creates an empty persistent canvas.
    ///
    /// Use the eventual presentation target's `format` to avoid conversion.
    pub fn new(context: &mut crate::Context, size: [u32; 2], format: wgpu::TextureFormat) -> Self {
        let size = [size[0].max(1), size[1].max(1)];
        Self {
            front: backing(context, size, format),
            back: backing(context, size, format),
            size,
            format,
            painted: false,
        }
    }

    /// `size` returns the canvas dimensions in pixels.
    pub fn size(&self) -> [u32; 2] {
        self.size
    }

    /// `front` returns the image containing the current canvas pixels.
    pub fn front(&self) -> &Image {
        &self.front
    }

    /// `present_to` copies the current canvas pixels into a render target.
    ///
    /// The copy is pixel-exact when the target matches [`Self::size`].
    pub fn present_to(&self, context: &mut crate::Context, target: &crate::RenderTarget) {
        let image = self.front();
        let source = Rect::new(0.0, 0.0, image.width(), image.height());
        let destination = Rect::new(0.0, 0.0, target.size[0] as f32, target.size[1] as f32);
        let mut builder = valo_dl::DisplayListBuilder::new();
        builder.draw_image_rect(
            image,
            source,
            destination,
            crate::context::EXACT_SAMPLING,
            &crate::context::copy_paint(),
        );
        context.render(&builder.build(), target);
    }

    /// `draw` applies a display list to the retained canvas pixels.
    ///
    /// Pass `None` to preserve previous pixels or `Some(color)` to replace
    /// them before drawing.
    pub fn draw(
        &mut self,
        context: &mut crate::Context,
        delta: &std::sync::Arc<DisplayList>,
        clear: Option<Color>,
    ) -> RenderStats {
        let mut frame = DisplayListBuilder::new();
        if clear.is_none() && self.painted {
            // WebGPU cannot unresolve prior pixels into the fresh MSAA target,
            // so an aligned 1:1 draw restores them without resampling.
            frame.draw_image_rect(
                &self.front,
                self.whole(),
                self.whole(),
                crate::context::EXACT_SAMPLING,
                &crate::context::copy_paint(),
            );
        }
        frame.draw_display_list(delta);
        let list = frame.build();

        // The scratch is always cleared; the restore draw above is what puts
        // the previous frame back. `Src` means it REPLACES rather than
        // composites, so a translucent canvas restores its own alpha instead
        // of accumulating it.
        let stats = context.render(
            &list,
            &self.back_target(clear.unwrap_or(Color::TRANSPARENT)),
        );
        std::mem::swap(&mut self.front, &mut self.back);
        self.painted = true;
        stats
    }

    /// `resize` reallocates the canvas and discards its contents.
    pub fn resize(&mut self, context: &mut crate::Context, size: [u32; 2]) {
        let size = [size[0].max(1), size[1].max(1)];
        if size == self.size {
            return;
        }
        *self = Self::new(context, size, self.format);
    }

    fn whole(&self) -> Rect {
        Rect::new(0.0, 0.0, self.size[0] as f32, self.size[1] as f32)
    }

    fn back_target(&self, clear: Color) -> RenderTarget<'_> {
        RenderTarget {
            view: self.back.view(),
            texture: self.back.texture(),
            format: self.format,
            size: self.size,
            clear: Some(clear),
        }
    }
}

fn backing(context: &mut crate::Context, size: [u32; 2], format: wgpu::TextureFormat) -> Image {
    let texture = context.device().create_texture(&wgpu::TextureDescriptor {
        label: Some("valo.canvas.backing"),
        size: wgpu::Extent3d {
            width: size[0],
            height: size[1],
            depth_or_array_layers: 1,
        },
        mip_level_count: 1,
        sample_count: 1,
        dimension: wgpu::TextureDimension::D2,
        format,
        // RENDER_ATTACHMENT to resolve into, TEXTURE_BINDING to restore and
        // blit from, COPY_SRC so a host can read the canvas back.
        usage: wgpu::TextureUsages::RENDER_ATTACHMENT
            | wgpu::TextureUsages::TEXTURE_BINDING
            | wgpu::TextureUsages::COPY_SRC,
        view_formats: &[],
    });
    context.import_image(texture, size)
}

#[cfg(test)]
mod tests {
    use super::*;
    use wgpu::Backend;
    use wgpu::CompositeAlphaMode as Mode;

    #[test]
    fn opaque_takes_the_first_mode_offered() {
        assert_eq!(
            alpha_mode_for(
                SurfaceAlpha::Opaque,
                Backend::Metal,
                &[Mode::Opaque, Mode::PostMultiplied]
            ),
            Mode::Opaque
        );
        assert_eq!(
            alpha_mode_for(SurfaceAlpha::Opaque, Backend::Vulkan, &[]),
            Mode::Auto
        );
    }

    #[test]
    fn transparent_takes_premultiplied_where_the_label_is_honest() {
        let offered = [
            Mode::Opaque,
            Mode::PreMultiplied,
            Mode::PostMultiplied,
            Mode::Inherit,
        ];
        assert_eq!(
            alpha_mode_for(SurfaceAlpha::Transparent, Backend::Vulkan, &offered),
            Mode::PreMultiplied
        );
        assert_eq!(
            alpha_mode_for(
                SurfaceAlpha::Transparent,
                Backend::Dx12,
                &[Mode::Opaque, Mode::PreMultiplied]
            ),
            Mode::PreMultiplied
        );
    }

    #[test]
    fn transparent_takes_metals_post_multiplied_and_no_one_elses() {
        let offered = [Mode::Opaque, Mode::PostMultiplied];
        assert_eq!(
            alpha_mode_for(SurfaceAlpha::Transparent, Backend::Metal, &offered),
            Mode::PostMultiplied
        );
        assert_eq!(
            alpha_mode_for(SurfaceAlpha::Transparent, Backend::Vulkan, &offered),
            Mode::Opaque
        );
    }

    #[test]
    fn transparent_on_webgpu_asks_without_consulting_the_capabilities() {
        assert_eq!(
            alpha_mode_for(
                SurfaceAlpha::Transparent,
                Backend::BrowserWebGpu,
                &[Mode::Opaque]
            ),
            Mode::PreMultiplied
        );
    }

    #[test]
    fn transparent_falls_back_to_inherit_then_to_opaque_and_says_so() {
        assert_eq!(
            alpha_mode_for(
                SurfaceAlpha::Transparent,
                Backend::Vulkan,
                &[Mode::Opaque, Mode::Inherit]
            ),
            Mode::Inherit
        );
        let degraded = alpha_mode_for(SurfaceAlpha::Transparent, Backend::Gl, &[Mode::Opaque]);
        assert_eq!(degraded, Mode::Opaque);
        assert_eq!(alpha_in_effect(degraded), SurfaceAlpha::Opaque);
        assert_eq!(alpha_in_effect(Mode::Inherit), SurfaceAlpha::Transparent);
    }
}
