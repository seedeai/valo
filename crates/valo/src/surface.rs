use valo_dl::{DisplayList, DisplayListBuilder, Image};
use valo_geometry::{Color, Rect};
use valo_renderer::{RenderStats, RenderTarget};

/// `Surface` manages a presentable native window or browser canvas.
///
/// Render each frame by calling `acquire`, [`crate::Context::render`], and
/// [`crate::Context::present`]. Valo selects a format that preserves its
/// CSS/Skia-compatible sRGB blending.
pub struct Surface {
    surface: wgpu::Surface<'static>,
    config: wgpu::SurfaceConfiguration,
    device: wgpu::Device,
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
        let surface = instance.create_surface(target)?;
        Ok(Self::from_wgpu_surface(surface, adapter, device, size))
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
        let surface = unsafe { instance.create_surface_unsafe(target)? };
        Ok(Self::from_wgpu_surface(surface, adapter, device, size))
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
        let caps = surface.get_capabilities(adapter);
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
            alpha_mode: caps.alpha_modes[0],
            view_formats: vec![],
        };
        surface.configure(device, &config);
        Self {
            surface,
            config,
            device: device.clone(),
        }
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

    /// `acquire` returns the next frame or `None` when this frame should be skipped.
    ///
    /// Lost or outdated surfaces are reconfigured and retried once.
    pub fn acquire(&mut self) -> Option<SurfaceFrame> {
        use wgpu::CurrentSurfaceTexture as C;
        for _ in 0..2 {
            match self.surface.get_current_texture() {
                C::Success(t) | C::Suboptimal(t) => {
                    let raw = t.texture.clone();
                    let view = raw.create_view(&wgpu::TextureViewDescriptor::default());
                    return Some(SurfaceFrame {
                        surface_texture: t,
                        raw,
                        view,
                        format: self.config.format,
                        size: [self.config.width, self.config.height],
                    });
                }
                C::Outdated | C::Lost => self.surface.configure(&self.device, &self.config),
                _ => return None,
            }
        }
        None
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
