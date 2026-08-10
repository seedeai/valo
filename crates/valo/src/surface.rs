use valo_geometry::Color;
use valo_renderer::RenderTarget;

/// The raw `MTLDevice*` behind a wgpu device (macOS) — hand it to a
/// `CAMetalLayer` so externally-owned drawable textures live on the same
/// GPU device as the renderer. Borrowed: valid while the device lives.
#[cfg(target_os = "macos")]
pub fn metal_device_of(device: &wgpu::Device) -> Option<std::ptr::NonNull<std::ffi::c_void>> {
    let hal_device = unsafe { device.as_hal::<wgpu::hal::api::Metal>() }?;
    let raw = objc2::rc::Retained::as_ptr(hal_device.raw_device());
    std::ptr::NonNull::new(raw.cast_mut().cast())
}

/// A caller-owned `MTLTexture` as a render target — the external-swapchain
/// route: the embedder drives the drawable cycle (acquire → render →
/// present), valo only draws. The `Offscreen` of foreign textures.
#[cfg(target_os = "macos")]
pub struct ExternalMetalTexture {
    texture: wgpu::Texture,
    view: wgpu::TextureView,
    /// The wrapped texture's pixel format, as the embedder created it.
    pub format: wgpu::TextureFormat,
    /// The wrapped texture's dimensions in pixels.
    pub size: [u32; 2],
}

#[cfg(target_os = "macos")]
impl ExternalMetalTexture {
    /// Wrap a raw `MTLTexture*` as a render target. The texture needs copy
    /// access for dst-reading blends and backdrops (a `CAMetalLayer`
    /// drawable: `framebufferOnly = false`).
    ///
    /// # Safety
    /// `texture` must be a valid `MTLTexture*` of exactly `size` in
    /// `format`, created on [`metal_device_of`]'s device.
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

    /// A render target over the wrapped texture. `clear` of `None` draws
    /// on top of whatever the embedder left there.
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

/// A raw `MTLTexture*` as a wgpu texture (retained for the wrapper's
/// lifetime) — the shared plumbing behind render targets and image
/// imports. `usage` must stay within what the texture was created for.
///
/// # Safety
/// `texture` must be a valid `MTLTexture*` of exactly `size` in `format`,
/// created on [`metal_device_of`]'s device.
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
    unsafe { device.create_texture_from_hal::<wgpu::hal::api::Metal>(hal_texture, &descriptor) }
}

/// A presentable surface (native window now; the web `<canvas>` constructor joins
/// in the platform milestone — wgpu's `SurfaceTarget` already speaks both, plan
/// 001 "Platform integration"). Owns configuration and resize; each frame is
/// `acquire → render → present`.
///
/// Format choice: prefers a NON-sRGB view format so blending happens in sRGB
/// space — the CSS/Skia-compatible look (linear blending is the deferred color
/// decision).
pub struct Surface {
    surface: wgpu::Surface<'static>,
    config: wgpu::SurfaceConfiguration,
    device: wgpu::Device,
}

impl Surface {
    /// Configure a swapchain over a window-like `target`. Picks a non-sRGB
    /// surface format so blending stays in sRGB space.
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

    /// [`Surface::new`] over a RAW platform target — the constructor for
    /// embedders that hold native handles rather than window types (a
    /// `CAMetalLayer*` from a C API, an `HWND`, …).
    ///
    /// # Safety
    /// The raw handle must be valid and outlive this surface.
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

    /// Shared tail of both constructors: pick a non-sRGB format (sRGB-space
    /// blending, the CSS/Skia look) and configure.
    fn from_wgpu_surface(
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
            // COPY_SRC: advanced blends snapshot the resolved target mid-frame.
            usage: wgpu::TextureUsages::RENDER_ATTACHMENT | wgpu::TextureUsages::COPY_SRC,
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

    /// Reconfigure the swapchain after the window changed size.
    pub fn resize(&mut self, size: [u32; 2]) {
        self.config.width = size[0].max(1);
        self.config.height = size[1].max(1);
        self.surface.configure(&self.device, &self.config);
    }

    /// The configured swapchain size in pixels.
    pub fn size(&self) -> [u32; 2] {
        [self.config.width, self.config.height]
    }

    /// The chosen swapchain format.
    pub fn format(&self) -> wgpu::TextureFormat {
        self.config.format
    }

    /// Acquire the next swapchain frame. `Outdated`/`Lost` reconfigure and
    /// retry once (the resize race); `None` means skip this frame (timeout /
    /// occluded window).
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

/// One acquired swapchain frame: make a [`RenderTarget`], render, `present`.
pub struct SurfaceFrame {
    surface_texture: wgpu::SurfaceTexture,
    raw: wgpu::Texture,
    view: wgpu::TextureView,
    /// This frame's pixel format (the surface's).
    pub format: wgpu::TextureFormat,
    /// This frame's dimensions in pixels.
    pub size: [u32; 2],
}

impl SurfaceFrame {
    /// A render target over this frame. `clear` of `None` preserves the
    /// swapchain texture's existing contents.
    pub fn target(&self, clear: Option<Color>) -> RenderTarget<'_> {
        RenderTarget {
            view: &self.view,
            texture: &self.raw,
            format: self.format,
            size: self.size,
            clear,
        }
    }

    /// Hand the frame to the compositor, consuming it.
    pub fn present(self) {
        self.surface_texture.present();
    }
}

/// An offscreen render target — headless tests, snapshots, and the high-res
/// export path (render at N× then read back; no special machinery).
pub struct Offscreen {
    texture: wgpu::Texture,
    view: wgpu::TextureView,
    /// Always [`Offscreen::FORMAT`].
    pub format: wgpu::TextureFormat,
    /// The target's dimensions in pixels.
    pub size: [u32; 2],
}

impl Offscreen {
    /// The format offscreen targets always use — readback and the PNG
    /// encoders downstream expect it.
    pub const FORMAT: wgpu::TextureFormat = wgpu::TextureFormat::Rgba8Unorm;

    /// Allocate an offscreen target of `size`, renderable and copyable.
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

    /// A render target over this texture.
    pub fn target(&self, clear: Option<Color>) -> RenderTarget<'_> {
        RenderTarget {
            view: &self.view,
            texture: &self.texture,
            format: self.format,
            size: self.size,
            clear,
        }
    }

    /// The underlying texture, for read-back or further GPU work.
    pub fn texture(&self) -> &wgpu::Texture {
        &self.texture
    }
}
