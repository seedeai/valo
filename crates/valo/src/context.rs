use valo_dl::{DisplayList, Image};
use valo_renderer::{ImageDesc, MemoryReport, RenderStats, RenderTarget, RendererCore};

/// One per `wgpu::Device`: owns every GPU-side cache (pipelines, per-frame
/// arenas — later: atlases, render-target pool, image registry). Stateless with
/// respect to content; `&mut self` renders one frame at a time, no internal
/// locks. `wgpu::Device`/`Queue` are internally refcounted, so hosts keep their
/// own clones freely.
pub struct Context {
    renderer: RendererCore,
    queue: wgpu::Queue,
}

impl Context {
    /// Build a context on a host-owned device and queue. Both are cloned
    /// (wgpu handles are `Arc`s), so the host keeps using its own.
    pub fn new(device: wgpu::Device, queue: wgpu::Queue) -> Self {
        Self {
            renderer: RendererCore::new(device, queue.clone()),
            queue,
        }
    }

    /// Replay `dl` into `target`. Submits exactly one command buffer.
    /// What valo holds between frames — per-pool counts and byte estimates
    /// plus wgpu's own counters (the `counters` feature). The debug-HUD API.
    pub fn memory_report(&self) -> MemoryReport {
        self.renderer.memory_report()
    }

    /// Replay `dl` into `target`, returning this frame's stats. Submits
    /// exactly one command buffer.
    pub fn render(&mut self, dl: &DisplayList, target: &RenderTarget) -> RenderStats {
        self.renderer.render(dl, target)
    }

    /// Register the fonts glyph runs rasterize through (once, after the
    /// Skip `.notdef` in every text tier so unresolved chars render blank
    /// instead of tofu boxes — OPT-IN; the default draws the box, like
    /// Skia. Pair with [`valo_text::FontDemand`] reporting so hidden
    /// misses surface through the API instead of pixels.
    pub fn set_hide_missing_glyphs(&mut self, hide: bool) {
        self.renderer.set_hide_missing_glyphs(hide);
    }

    /// The registered collection (`None` before `set_fonts`) — overlays
    /// Text tier thresholds (device px): masks < `sdf_min` ≤ SDF <
    /// `path_min` ≤ outlines. Defaults are Skia's; lower `sdf_min` toward
    /// ~18–64 for Skia's zoom-heavy trade (fewer rasters, softer small text).
    pub fn set_text_tiers(&mut self, tiers: valo_renderer::TextTiers) {
        self.renderer.set_text_tiers(tiers);
    }

    /// Gesture switch, OPT-IN (default off — every raster stays eager):
    /// while held, an SDF glyph missing at the wanted size draws through
    /// its nearest resident size, scaled (soft, like mid-pinch Chrome),
    /// instead of rasterizing; glyphs with no resident size still raster.
    /// The HOST owns the timing: hold while camera input streams, clear on
    /// idle and re-render — valo keeps no clocks. `RenderStats::
    /// held_rasters` counts the skips (nonzero at idle = a stuck hold).
    pub fn set_text_raster_hold(&mut self, held: bool) {
        self.renderer.set_text_raster_hold(held);
    }

    /// A camera gesture is in flight: the list raster cache reuses existing
    /// textures at any scale instead of refilling; clear on settle
    /// (pairs with [`Self::set_text_raster_hold`]).
    pub fn set_raster_hold(&mut self, held: bool) {
        self.renderer.set_raster_hold(held);
    }

    // Export-only accessors; the readback path doesn't exist on wasm.
    #[cfg(not(target_arch = "wasm32"))]
    pub(crate) fn device(&self) -> &wgpu::Device {
        self.renderer.device()
    }

    #[cfg(not(target_arch = "wasm32"))]
    pub(crate) fn queue_handle(&self) -> wgpu::Queue {
        self.queue.clone()
    }

    /// RGBA8 pixels → retained [`Image`]: premultiplied at the boundary,
    /// full mip chain rendered on the GPU (posters downscale constantly).
    /// Dropping the returned handle is the whole lifetime story.
    pub fn upload_image(&mut self, desc: ImageDesc, pixels: &[u8]) -> Image {
        self.renderer.images().upload(desc, pixels)
    }

    /// An externally-rendered texture as a drawable [`Image`], zero-copy —
    /// the native sibling of the web `ImageBitmap` path. No mips: sources
    /// that re-render per frame would only throw them away.
    pub fn import_image(&mut self, texture: wgpu::Texture, size: [u32; 2]) -> Image {
        self.renderer.images().finish_external(texture, size, 1)
    }

    /// Web: let the BROWSER decode. Copies an `ImageBitmap` straight into a
    /// texture (`copy_external_image_to_texture` — off-main-thread decode,
    /// no wasm-side pixel copy), then builds mips like any upload.
    /// The bitmap should be premultiplied (createImageBitmap default).
    #[cfg(target_arch = "wasm32")]
    pub fn upload_image_bitmap(&mut self, bitmap: &web_sys::ImageBitmap, mips: bool) -> Image {
        let size = [bitmap.width(), bitmap.height()];
        self.upload_external_image(
            wgpu::ExternalImageSource::ImageBitmap(bitmap.clone()),
            size,
            mips,
        )
    }

    /// Any WebGPU-copyable DOM source — `<img>`, `<canvas>`, `<video>`,
    /// `ImageBitmap`, `OffscreenCanvas`, `ImageData` — into a fresh [`Image`].
    ///
    /// The copy is SYNCHRONOUS, which is what lets a Canvas2D `drawImage`
    /// shim exist at all. The caller owns the source's readiness: an
    /// undecoded `<img>` makes this throw, where Canvas2D silently draws
    /// nothing.
    #[cfg(target_arch = "wasm32")]
    pub fn upload_external_image(
        &mut self,
        source: wgpu::ExternalImageSource,
        size: [u32; 2],
        mips: bool,
    ) -> Image {
        self.upload_external_image_region(source, [0, 0], size, mips)
    }

    /// The same, reading only `size` pixels starting at `origin` in the
    /// source. `putImageData` with a dirty rectangle needs this: without it a
    /// one-pixel update to a 4K `ImageData` would copy every one of its
    /// ~32 MiB and retain a full-size texture to sample one texel from.
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

    /// Re-copy a changed source into an image that already exists. A
    /// `<video>` produces a new frame every tick, and minting a new [`Image`]
    /// each time would throw away the renderer's per-image bind-group cache
    /// and leave a texture per frame for the pool to reclaim. Same handle,
    /// same bind group, new pixels.
    ///
    /// `false` when the source no longer matches the image's dimensions —
    /// the caller has to upload afresh.
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
