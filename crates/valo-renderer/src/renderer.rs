use valo_dl::DisplayList;
use valo_geometry::Color;

use crate::contours::ContourCache;
use crate::encoder::Encoder;
use crate::frame::FramePlan;
use crate::glyphs::GlyphStore;
use crate::gpu_timer::GpuTimer;
use crate::host_buffer::{Flushed, HostBuffer};
use crate::images::ImageStore;
use crate::pipelines::PipelineCache;
use crate::planner::{Caches, EmitterServices, LinearSamplers, Planner};
use crate::pool::TargetPool;

/// `RenderTarget` is the destination for one render operation.
///
/// `texture` must be the resource behind `view` and match `format` and `size`.
/// Advanced blends, backdrop filters and keeping the target's pixels
/// (`clear: None`) require `COPY_SRC` texture usage.
pub struct RenderTarget<'a> {
    /// `view` is the attachment into which Valo renders.
    pub view: &'a wgpu::TextureView,
    /// `texture` is the resource behind `view`.
    pub texture: &'a wgpu::Texture,
    /// `format` is the pixel format exposed by `view`.
    pub format: wgpu::TextureFormat,
    /// `size` is the renderable area in pixels.
    pub size: [u32; 2],
    /// `clear` replaces existing pixels when set and preserves them when `None`.
    ///
    /// Preserving costs a copy of the target and a full-size draw: valo draws
    /// into multisample scratch that never keeps a picture between frames,
    /// so it copies the target's pixels out and draws them back first.
    pub clear: Option<Color>,
}

impl RenderTarget<'_> {
    /// `assert_can_keep_pixels` stops a render that keeps the target's
    /// pixels on a target valo cannot copy them out of.
    fn assert_can_keep_pixels(&self) {
        assert!(
            self.clear.is_some() || self.texture.usage().contains(wgpu::TextureUsages::COPY_SRC),
            "a target drawn over without clearing (`clear: None`) needs `COPY_SRC` usage: \
             valo copies its pixels out and draws them back"
        );
    }
}

/// `RenderStats` reports the work performed by one render operation.
#[derive(Clone, Copy, Debug, Default)]
pub struct RenderStats {
    /// `ops` is the number of replayed operations, including nested lists.
    pub ops: u32,
    /// `draws` is the number of logical draws remaining after culling.
    pub draws: u32,
    /// `clips` is the number of encoded clip operations.
    pub clips: u32,
    /// `culled` is the number of draws skipped because their bounds miss the target.
    pub culled: u32,
    /// `layers_rendered` is the number of offscreen layers actually rendered.
    pub layers_rendered: u32,
    /// `layers_elided` is the number of save layers applied without an offscreen target.
    pub layers_elided: u32,
    /// `snapshots` is the number of target-region copies used by destination reads.
    pub snapshots: u32,
    /// `backdrops` is the number of backdrop regions that ran a blur chain.
    pub backdrops: u32,
    /// `shared_backdrops` is the number of backdrop regions that reused a shared blur.
    pub shared_backdrops: u32,
    /// `filter_passes` is the number of encoded image-filter passes.
    pub filter_passes: u32,
    /// `text_tiers` contains glyph-run counts for `[bitmap mask, SDF, outline]`.
    pub text_tiers: [u32; 3],
    /// `glyph_rasters` is the number of glyphs rasterized after cache misses.
    pub glyph_rasters: u32,
    /// `raster_quads` is the number of cached display lists drawn as image quads.
    pub raster_quads: u32,
    /// `raster_fills` is the number of display lists rendered into cache textures.
    pub raster_fills: u32,
    /// `atlas_gcs` is the number of full glyph-atlas collections.
    pub atlas_gcs: u32,
    /// `held_rasters` is the number of bitmap or SDF misses served by a resident size.
    pub held_rasters: u32,
    /// `opaque_reordered` is the number of opaque draws moved earlier for depth culling.
    pub opaque_reordered: u32,
    /// `gpu_ms` is the previous resolved frame's GPU time in milliseconds.
    ///
    /// It is zero until a timestamp resolves or when timestamps are unavailable.
    pub gpu_ms: f32,
    /// `blocks_created` is the number of transient upload blocks allocated this frame.
    pub blocks_created: u32,
    /// `cpu_ms` is total CPU time spent in rendering and submission.
    pub cpu_ms: f32,
    /// `draw_calls` is the number of encoded GPU draw commands.
    ///
    /// One logical draw may require several commands, while batched glyphs may
    /// share one.
    pub draw_calls: u32,
    /// `render_passes` is the number of encoded render passes.
    pub render_passes: u32,
    /// `pipeline_switches` is the number of encoded pipeline changes.
    pub pipeline_switches: u32,
    /// `vertex_bytes` is the number of transient vertex bytes uploaded.
    pub vertex_bytes: u64,
    /// `uniform_bytes` is the number of transient uniform bytes uploaded.
    pub uniform_bytes: u64,
    /// `plan_ms` is the CPU time spent replaying and planning.
    pub plan_ms: f32,
    /// `encode_ms` is the CPU time spent compiling, uploading, encoding, and submitting.
    pub encode_ms: f32,
}

/// `RendererCore` replays display lists on a host-owned wgpu device.
///
/// Hosts normally use the `valo` crate's `Context` instead. This type is the
/// GPU core that context wraps: it owns caches and pipelines, and holds no
/// application content of its own.
pub struct RendererCore {
    device: wgpu::Device,
    queue: wgpu::Queue,
    /// What the planner's step emitter borrows each frame.
    services: EmitterServices,
    pool: TargetPool,
    /// What planning reads and fills each frame.
    caches: Caches,
    timer: GpuTimer,
}

impl RendererCore {
    /// `new` creates a renderer from a host-owned device and queue.
    pub fn new(device: wgpu::Device, queue: wgpu::Queue) -> Self {
        let glyphs = GlyphStore::new(&device, &queue);
        Self::with_glyphs(device, queue, glyphs)
    }

    /// `with_glyph_raster` creates a renderer whose glyphs `raster` shapes, for a host
    /// whose glyph shapes come from elsewhere than the fonts' own bytes.
    pub fn with_glyph_raster(
        device: wgpu::Device,
        queue: wgpu::Queue,
        raster: Box<dyn valo_text::GlyphRaster>,
    ) -> Self {
        let glyphs = GlyphStore::with_raster(&device, &queue, raster);
        Self::with_glyphs(device, queue, glyphs)
    }

    fn with_glyphs(device: wgpu::Device, queue: wgpu::Queue, glyphs: GlyphStore) -> Self {
        let host = HostBuffer::new(&device);
        let pipelines = PipelineCache::new(
            &device,
            host.bind_group_layout(),
            host.kernel_bind_group_layout(),
        );
        Self {
            services: EmitterServices {
                host,
                pipelines,
                images: ImageStore::new(&device, &queue),
                ramps: crate::ramps::RampCache::new(),
                samplers: LinearSamplers::new(&device),
            },
            pool: TargetPool::new(&device),
            caches: Caches {
                glyphs,
                contours: ContourCache::new(),
                rasters: crate::raster::ListRasterCache::new(),
            },
            timer: GpuTimer::new(&device, &queue),
            device,
            queue,
        }
    }

    /// `image_context` shares image creation without sharing the mutable drawing caches.
    pub fn image_context(&self) -> crate::ImageContext {
        self.services.images.context.clone()
    }

    /// `images` returns the image store used for uploads and sampling.
    pub fn images(&mut self) -> &mut ImageStore {
        &mut self.services.images
    }

    /// `device` returns the device used by this renderer.
    pub fn device(&self) -> &wgpu::Device {
        &self.device
    }

    /// `set_text_tiers` controls how text is rendered across font-size ranges.
    ///
    /// Valo uses bitmap masks below `sdf_min`, SDF below `path_min`, and
    /// outlines above it. The defaults suit normal use; override them only for
    /// specialized scaling or zoom behavior.
    pub fn set_text_tiers(&mut self, tiers: crate::TextTiers) {
        self.caches.glyphs.tiers = tiers;
    }

    /// `set_hide_missing_glyphs` controls whether unresolved characters render blank.
    ///
    /// By default, unresolved characters render the font's `.notdef` glyph,
    /// usually a "tofu" box. This is common when CJK fallback fonts are missing.
    /// Use [`valo_text::FontDemand`] to detect characters hidden by this option.
    pub fn set_hide_missing_glyphs(&mut self, hide: bool) {
        self.caches.glyphs.set_hide_missing_glyphs(hide);
    }

    /// `set_text_raster_hold` allows existing text rasters to stand in for missing sizes.
    ///
    /// This applies to bitmap-mask and SDF text, not vector outlines. It is
    /// useful during rapid zooming: enable it while the gesture is active and
    /// clear it afterward so the next frame renders sharply.
    pub fn set_text_raster_hold(&mut self, held: bool) {
        self.caches.glyphs.set_text_raster_hold(held);
    }

    /// `set_raster_hold` allows cached display-list textures to be reused at any scale.
    ///
    /// This is useful during rapid zooming: enable it when the gesture starts
    /// and clear it when the view settles so caches refill at the final scale.
    pub fn set_raster_hold(&mut self, held: bool) {
        self.caches.rasters.set_hold(held);
    }

    /// `render` draws a display list into a target and returns frame statistics.
    ///
    /// Each call submits one command buffer.
    pub fn render(&mut self, list: &DisplayList, target: &RenderTarget) -> RenderStats {
        #[cfg(feature = "trace")]
        let _span = tracing::info_span!("valo.render", draws = list.draw_count()).entered();
        target.assert_can_keep_pixels();
        let mut frame = self.begin_frame();
        let plan = frame.plan(list, target);
        let flushed = frame.upload();
        frame.encode_and_submit(&plan, flushed);
        frame.end()
    }

    /// `memory_report` returns resource counts and estimated GPU memory usage.
    ///
    /// The `counters` feature adds the counters reported by wgpu.
    pub fn memory_report(&self) -> crate::MemoryReport {
        crate::MemoryReport {
            images: self.services.images.report(),
            atlas: self.caches.glyphs.report_atlas(),
            targets: self.pool.report(),
            host_buffer: self.services.host.report(),
            contours: self.caches.contours.report(),
            glyph_paths: self.caches.glyphs.report_paths(),
            ramps: self.services.ramps.report(),
            raster_cache: self.caches.rasters.report(),
            wgpu: crate::report::wgpu_counters(&self.device),
        }
    }

    /// `begin_frame` starts one render call's [`Frame`].
    fn begin_frame(&mut self) -> Frame<'_> {
        let started = web_time::Instant::now();
        self.services.host.begin_frame();
        Frame {
            core: self,
            started,
            planned: None,
            stats: RenderStats::default(),
        }
    }
}

/// `Frame` is one render call, in the order its methods are called:
/// planning into the host buffer the frame began, uploading what planning
/// wrote on the CPU, compiling the plan's pipelines and encoding and
/// submitting one command buffer, and ending every cache's frame. Each phase
/// records its own stats.
struct Frame<'r> {
    core: &'r mut RendererCore,
    started: web_time::Instant,
    /// When planning finished.
    planned: Option<web_time::Instant>,
    stats: RenderStats,
}

impl Frame<'_> {
    /// `plan` walks `list` into `target`: the passes the encoder replays.
    fn plan(&mut self, list: &DisplayList, target: &RenderTarget) -> FramePlan {
        #[cfg(feature = "trace")]
        let _span = tracing::info_span!("valo.plan").entered();
        let core = &mut *self.core;
        let plan = Planner::plan(
            &core.device,
            &mut core.services,
            &mut core.pool,
            &mut core.caches,
            list,
            target,
        );
        self.planned = Some(web_time::Instant::now());
        self.stats = plan.stats;
        self.stats.render_passes = plan.passes.len() as u32;
        plan
    }

    /// `upload` moves what planning wrote on the CPU to the GPU: the glyph
    /// atlas pages' dirty regions, new gradient ramps and the host buffer's
    /// blocks. The receipt
    /// goes to the submit.
    fn upload(&mut self) -> Flushed {
        self.core.caches.glyphs.flush_uploads();
        self.core.services.ramps.flush_uploads(&self.core.queue);
        let flushed = self.core.services.host.flush(&self.core.queue);
        self.stats.uniform_bytes = flushed.uniform_bytes;
        self.stats.vertex_bytes = flushed.vertex_bytes;
        self.stats.blocks_created = flushed.blocks_created;
        flushed
    }

    /// `encode_and_submit` compiles every pipeline the plan names, records
    /// the plan into one command buffer, with the frame's GPU timestamps,
    /// submits it, and hands the host buffer the upload's receipt back once
    /// the frame is submitted.
    fn encode_and_submit(&mut self, plan: &FramePlan, flushed: Flushed) {
        #[cfg(feature = "trace")]
        let _span = tracing::info_span!("valo.encode", passes = plan.passes.len()).entered();
        let core = &mut *self.core;
        let pipelines = core
            .services
            .pipelines
            .compile(&core.device, plan.pipeline_keys());
        let mut command_encoder =
            core.device
                .create_command_encoder(&wgpu::CommandEncoderDescriptor {
                    label: Some("valo.frame"),
                });
        let encoder = Encoder {
            host: &core.services.host,
            pipelines: &pipelines,
        };
        let encoded = encoder.encode(&mut command_encoder, plan, &core.timer);
        self.stats.draw_calls = encoded.draw_calls;
        self.stats.pipeline_switches = encoded.pipeline_switches;
        core.timer.end_frame(&mut command_encoder);
        core.queue.submit(std::iter::once(command_encoder.finish()));
        core.services.host.after_submit(flushed);
        core.timer.after_submit();
    }

    /// `end` ends every cache's frame and returns the frame's stats.
    fn end(self) -> RenderStats {
        let core = self.core;
        let glyphs = core.caches.glyphs.end_frame();
        core.pool.end_frame();
        core.services.images.end_frame();
        core.services.ramps.end_frame();
        core.caches.contours.end_frame();
        core.caches.rasters.end_frame();
        let planned = self.planned.unwrap_or(self.started);
        RenderStats {
            glyph_rasters: glyphs.rasters,
            atlas_gcs: glyphs.atlas_gcs,
            held_rasters: glyphs.held_rasters,
            plan_ms: (planned - self.started).as_secs_f32() * 1000.0,
            encode_ms: planned.elapsed().as_secs_f32() * 1000.0,
            cpu_ms: self.started.elapsed().as_secs_f32() * 1000.0,
            gpu_ms: core.timer.latest_ms(&core.device),
            ..self.stats
        }
    }
}
