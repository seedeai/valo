use valo_dl::{BlendMode, DisplayList, Filter, Image, MipmapMode, Paint, Sampling, TileMode};
use valo_geometry::Color;
use valo_renderer::{ImageDesc, MemoryReport, RenderStats, RenderTarget, RendererCore};

/// Context renders display lists on a host-owned wgpu device.
///
/// Create one context per device. The host may freely retain clones of the
/// device and queue handles.
pub struct Context {
    renderer: RendererCore,
    queue: wgpu::Queue,
}

impl Context {
    /// `new` creates a context from a host-owned device and queue.
    pub fn new(device: wgpu::Device, queue: wgpu::Queue) -> Self {
        Self {
            renderer: RendererCore::new(device, queue.clone()),
            queue,
        }
    }

    /// `memory_report` returns resource counts and estimated GPU memory usage.
    ///
    /// The `counters` feature adds the counters reported by wgpu.
    pub fn memory_report(&self) -> MemoryReport {
        self.renderer.memory_report()
    }

    /// `render` draws a display list into a target and returns frame statistics.
    ///
    /// Each call submits one command buffer.
    pub fn render(&mut self, dl: &DisplayList, target: &RenderTarget) -> RenderStats {
        self.renderer.render(dl, target)
    }

    /// `set_hide_missing_glyphs` controls whether unresolved characters render blank.
    ///
    /// By default, unresolved characters render the font's `.notdef` glyph,
    /// usually a "tofu" box. This is common when CJK fallback fonts are missing.
    /// Use [`crate::FontDemand`] to detect characters hidden by this option.
    pub fn set_hide_missing_glyphs(&mut self, hide: bool) {
        self.renderer.set_hide_missing_glyphs(hide);
    }

    /// `set_text_tiers` controls how text is rendered across font-size ranges.
    ///
    /// Valo uses bitmap masks below `sdf_min`, SDF below `path_min`, and
    /// outlines above it. The defaults suit normal use; override them only for
    /// specialized scaling or zoom behavior.
    pub fn set_text_tiers(&mut self, tiers: valo_renderer::TextTiers) {
        self.renderer.set_text_tiers(tiers);
    }

    /// `set_text_raster_hold` allows existing text rasters to stand in for missing sizes.
    ///
    /// This applies to bitmap-mask and SDF text, not vector outlines. It is
    /// useful during rapid zooming: enable it while the gesture is active and
    /// clear it afterward so the next frame renders sharply.
    pub fn set_text_raster_hold(&mut self, held: bool) {
        self.renderer.set_text_raster_hold(held);
    }

    /// `set_raster_hold` allows cached display-list textures to be reused at any scale.
    ///
    /// This is useful during rapid zooming: enable it when the gesture starts
    /// and clear it when the view settles so caches refill at the final scale.
    pub fn set_raster_hold(&mut self, held: bool) {
        self.renderer.set_raster_hold(held);
    }

    /// `device` returns the device used by this context.
    pub fn device(&self) -> &wgpu::Device {
        self.renderer.device()
    }

    // Native readback needs the queue; wasm has no blocking readback path.
    #[cfg(not(target_arch = "wasm32"))]
    pub(crate) fn queue_handle(&self) -> wgpu::Queue {
        self.queue.clone()
    }

    /// `present` hands a rendered surface frame to the compositor.
    pub fn present(&self, frame: crate::SurfaceFrame) {
        frame.present(&self.queue);
    }

    /// `upload_image` uploads RGBA8 pixels and returns a retained [`Image`].
    ///
    /// Alpha conversion and mip generation follow the supplied [`ImageDesc`].
    ///
    /// # Panics
    ///
    /// Panics if `pixels` does not contain exactly four bytes per pixel.
    pub fn upload_image(&mut self, desc: ImageDesc, pixels: &[u8]) -> Image {
        self.renderer.images().upload(desc, pixels)
    }

    /// `import_image` wraps an existing texture as an [`Image`] without copying it.
    ///
    /// Imported images have one mip level.
    pub fn import_image(&mut self, texture: wgpu::Texture, size: [u32; 2]) -> Image {
        self.renderer.images().finish_external(texture, size, 1)
    }

    /// `upload_image_bitmap` uploads a browser-decoded `ImageBitmap`.
    ///
    /// Pixels are copied directly into a retained [`Image`] without passing
    /// through WebAssembly memory. Set `mips` when the image will be downscaled.
    #[cfg(target_arch = "wasm32")]
    pub fn upload_image_bitmap(&mut self, bitmap: &web_sys::ImageBitmap, mips: bool) -> Image {
        let size = [bitmap.width(), bitmap.height()];
        self.upload_external_image(
            wgpu::ExternalImageSource::ImageBitmap(bitmap.clone()),
            size,
            mips,
        )
    }

    /// `upload_external_image` uploads a WebGPU-copyable DOM source.
    ///
    /// Supported sources include `<img>`, `<canvas>`, `<video>`, `ImageBitmap`,
    /// `OffscreenCanvas`, and `ImageData`. The source must be ready when called;
    /// unlike Canvas2D, an undecoded image is not silently ignored.
    #[cfg(target_arch = "wasm32")]
    pub fn upload_external_image(
        &mut self,
        source: wgpu::ExternalImageSource,
        size: [u32; 2],
        mips: bool,
    ) -> Image {
        self.upload_external_image_region(source, [0, 0], size, mips)
    }

    /// `upload_external_image_region` uploads a rectangular region of a DOM source.
    ///
    /// `origin` is the region's top-left source coordinate, and `size`
    /// determines both the copied region and the returned image dimensions.
    #[cfg(target_arch = "wasm32")]
    pub fn upload_external_image_region(
        &mut self,
        source: wgpu::ExternalImageSource,
        origin: [u32; 2],
        size: [u32; 2],
        mips: bool,
    ) -> Image {
        let mip_levels = if mips {
            32 - size[0].max(size[1]).max(1).leading_zeros()
        } else {
            1
        };
        let texture = self
            .renderer
            .images()
            .create_image_texture(size, mip_levels);
        copy_external_image(&self.queue, source, origin, &texture, size);
        self.renderer
            .images()
            .finish_external(texture, size, mip_levels)
    }

    /// `refresh_external_image` replaces the pixels of an existing [`Image`].
    ///
    /// Use it for changing sources such as canvas or video frames to preserve
    /// the image handle and its caches. It returns `false` without copying when
    /// `size` differs from the existing image dimensions.
    #[cfg(target_arch = "wasm32")]
    pub fn refresh_external_image(
        &mut self,
        image: &Image,
        source: wgpu::ExternalImageSource,
        size: [u32; 2],
    ) -> bool {
        if image.size() != size {
            return false;
        }
        copy_external_image(&self.queue, source, [0, 0], image.texture(), size);
        self.renderer.images().regenerate_mips(image);
        true
    }
}

#[cfg(target_arch = "wasm32")]
fn copy_external_image(
    queue: &wgpu::Queue,
    source: wgpu::ExternalImageSource,
    origin: [u32; 2],
    texture: &wgpu::Texture,
    size: [u32; 2],
) {
    queue.copy_external_image_to_texture(
        &wgpu::CopyExternalImageSourceInfo {
            source,
            origin: wgpu::Origin2d {
                x: origin[0],
                y: origin[1],
            },
            flip_y: false,
        },
        wgpu::CopyExternalImageDestInfo {
            texture,
            mip_level: 0,
            origin: wgpu::Origin3d::ZERO,
            aspect: wgpu::TextureAspect::All,
            color_space: wgpu::PredefinedColorSpace::Srgb,
            premultiplied_alpha: true,
        },
        wgpu::Extent3d {
            width: size[0],
            height: size[1],
            depth_or_array_layers: 1,
        },
    );
}

/// `EXACT_SAMPLING` preserves source texels during 1:1 canvas copies.
///
/// [`crate::PersistentCanvas`] provides pixel alignment; nearest filtering,
/// clamping, and disabled mipmaps prevent sampling from altering the copy.
pub(crate) const EXACT_SAMPLING: Sampling = Sampling {
    filter: Filter::Nearest,
    mipmap: MipmapMode::None,
    tile_x: TileMode::Clamp,
    tile_y: TileMode::Clamp,
};

/// `copy_paint` creates an untinted replacement paint for image copies.
///
/// `fs_image` multiplies samples by the paint color, so the default black would
/// erase the image. `Src` replaces the destination without accumulating alpha.
pub(crate) fn copy_paint() -> Paint {
    Paint {
        color: Color::WHITE,
        blend_mode: BlendMode::Src,
        ..Paint::default()
    }
}
