use valo_dl::{DisplayList, DisplayListBuilder, Image};
use valo_geometry::{Color, Rect};
use valo_renderer::{RenderStats, RenderTarget};

/// `metal_device_of` returns the raw `MTLDevice*` behind a wgpu device.
///
/// Use it to configure a `CAMetalLayer` whose textures Valo will render into.
/// It returns `None` for non-Metal backends. The pointer is borrowed and
/// remains valid while `device` lives.
#[cfg(target_os = "macos")]
pub fn metal_device_of(device: &wgpu::Device) -> Option<std::ptr::NonNull<std::ffi::c_void>> {
    let hal_device = unsafe { device.as_hal::<wgpu::hal::api::Metal>() }?;
    let raw = objc2::rc::Retained::as_ptr(hal_device.raw_device());
    std::ptr::NonNull::new(raw.cast_mut().cast())
}

/// `ExternalMetalTexture` wraps a caller-owned `MTLTexture` as a render target.
///
/// The embedder remains responsible for acquiring and presenting the texture.
#[cfg(target_os = "macos")]
pub struct ExternalMetalTexture {
    texture: wgpu::Texture,
    view: wgpu::TextureView,
    /// `format` is the wrapped texture's pixel format.
    pub format: wgpu::TextureFormat,
    /// `size` is the wrapped texture's dimensions in pixels.
    pub size: [u32; 2],
}

#[cfg(target_os = "macos")]
impl ExternalMetalTexture {
    /// `wrap` creates a render target from a raw `MTLTexture*`.
    ///
    /// Destination-reading blends and backdrop filters require copy access.
    /// For a `CAMetalLayer` drawable, set `framebufferOnly` to `false`.
    ///
    /// # Safety
    /// `texture` must point to a texture of exactly `size` and `format` created
    /// by the device returned from [`metal_device_of`].
    pub unsafe fn wrap(
        device: &wgpu::Device,
        texture: std::ptr::NonNull<std::ffi::c_void>,
        size: [u32; 2],
        format: wgpu::TextureFormat,
    ) -> Self {
        let texture = unsafe {
            wrap_metal_texture(
                device,
                texture,
                size,
                format,
                wgpu::TextureUsages::RENDER_ATTACHMENT | wgpu::TextureUsages::COPY_SRC,
            )
        };
        let view = texture.create_view(&wgpu::TextureViewDescriptor::default());
        Self {
            texture,
            view,
            format,
            size,
        }
    }

    /// `target` creates a render target over the wrapped texture.
    ///
    /// Pass `None` to preserve the texture's existing pixels.
    pub fn target(&self, clear: Option<Color>) -> RenderTarget<'_> {
        RenderTarget {
            view: &self.view,
            texture: &self.texture,
            format: self.format,
            size: self.size,
            clear,
        }
    }
}

/// `wrap_metal_texture` wraps a raw `MTLTexture*` as a wgpu texture.
///
/// The returned texture retains the Metal texture. `usage` must not exceed
/// the usages with which the original texture was created.
///
/// # Safety
/// `texture` must point to a texture of exactly `size` and `format` created by
/// the device returned from [`metal_device_of`].
#[cfg(target_os = "macos")]
pub unsafe fn wrap_metal_texture(
    device: &wgpu::Device,
    texture: std::ptr::NonNull<std::ffi::c_void>,
    size: [u32; 2],
    format: wgpu::TextureFormat,
    usage: wgpu::TextureUsages,
) -> wgpu::Texture {
    let raw = texture
        .cast::<objc2::runtime::ProtocolObject<dyn objc2_metal::MTLTexture>>()
        .as_ptr();
    let retained =
        unsafe { objc2::rc::Retained::retain(raw) }.expect("retaining a non-null MTLTexture");
    let hal_texture = unsafe {
        wgpu::hal::metal::Device::texture_from_raw(
            retained,
            format,
            objc2_metal::MTLTextureType::Type2D,
            1,
            1,
            wgpu::hal::CopyExtent {
                width: size[0],
                height: size[1],
                depth: 1,
            },
            // The HOST owns this texture's lifetime — we only retained it —
            // so wgpu must not run a destructor when its handle drops.
            None,
        )
    };
    let descriptor = wgpu::TextureDescriptor {
        label: Some("valo.external-metal-texture"),
        size: wgpu::Extent3d {
            width: size[0],
            height: size[1],
            depth_or_array_layers: 1,
        },
        mip_level_count: 1,
        sample_count: 1,
        dimension: wgpu::TextureDimension::D2,
        format,
        usage,
        view_formats: &[],
    };
    // wgpu 30 wants the state the texture arrives in. An imported target is
    // one valo is about to render into, and its previous contents are the
    // host's business, so COLOR_TARGET is the honest declaration —
    // UNINITIALIZED would license discarding pixels the host may still want.
    unsafe {
        device.create_texture_from_hal::<wgpu::hal::api::Metal>(
            hal_texture,
            &descriptor,
            wgpu::wgt::TextureUses::COLOR_TARGET,
        )
    }
}

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
