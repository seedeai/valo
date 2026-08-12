use valo_dl::DisplayList;
use valo_geometry::Color;

use crate::contours::ContourCache;
use crate::glyphs::GlyphStore;
use crate::gpu_timer::GpuTimer;
use crate::host_buffer::HostBuffer;
use crate::images::ImageStore;
use crate::pipelines::PipelineCache;
use crate::plan::{FramePlan, PassColor, PlannedPass, Planner};
use crate::pool::TargetPool;

/// Where one frame goes. Caller-owned — a swapchain view, an offscreen
/// texture, an export target; the renderer allocates only pooled scratch.
/// `texture` is the resolved image behind `view`: advanced blends snapshot
/// it mid-frame, so it needs `COPY_SRC` usage.
pub struct RenderTarget<'a> {
    pub view: &'a wgpu::TextureView,
    pub texture: &'a wgpu::Texture,
    pub format: wgpu::TextureFormat,
    pub size: [u32; 2],
    /// `Some` clears before drawing; `None` loads existing contents.
    pub clear: Option<Color>,
}

/// Per-frame counters — the stats line. Cheap enough to always collect.
#[derive(Clone, Copy, Debug, Default)]
pub struct RenderStats {
    /// Ops replayed (nested lists included).
    pub ops: u32,
    /// Logical draws encoded after culling (a path fill counts once).
    pub draws: u32,
    /// Clips encoded.
    pub clips: u32,
    /// Draws skipped by record-time bounds vs. the viewport.
    pub culled: u32,
    /// Offscreen layer passes actually rendered (incl. implicit ones that
    /// desugar advanced-blend draws).
    pub layers_rendered: u32,
    /// Save layers that never became a texture — alpha rode the children
    /// (the best layer is the one never allocated).
    pub layers_elided: u32,
    /// Pass breaks for dst-reading blends and backdrops (each = one region
    /// copy of the target).
    pub snapshots: u32,
    /// Backdrop tiles that ran a blur chain (each = one pass break).
    pub backdrops: u32,
    /// Backdrop tiles that REUSED a shared blur — no break, no filter work.
    pub shared_backdrops: u32,
    /// Gaussian filter passes this frame (downsample + H + V per blur).
    pub filter_passes: u32,
    /// Glyph runs per text tier this frame: [mask, sdf, path].
    pub text_tiers: [u32; 3],
    /// Glyph-cache misses this frame (each = one swash raster). Warm
    /// frames read 0; sustained nonzero at a still camera is a cache bug.
    pub glyph_rasters: u32,
    /// Cached sub-lists drawn as ONE sampled quad this frame —
    /// the raster cache's payoff column.
    pub raster_quads: u32,
    /// Sub-lists rendered into cache textures this frame (extra passes;
    /// bounded by the per-frame fill quota).
    pub raster_fills: u32,
    /// Wholesale atlas GCs this frame; nonzero on warm frames means the
    /// live glyph set no longer fits the pages (thrash).
    pub atlas_gcs: u32,
    /// SDF rasters skipped by the host's text-raster hold this frame (each
    /// drew a scaled stand-in instead). Nonzero while idle = a stuck hold.
    pub held_rasters: u32,
    /// Opaque draw units hoisted out of painter order to the front of their
    /// segment chunk — each is overdraw early-z now culls.
    pub opaque_reordered: u32,
    /// GPU time of the PREVIOUS frame, ms (0 until one resolves, or when
    /// the device lacks TIMESTAMP_QUERY).
    pub gpu_ms: f32,
    /// HostBuffer blocks created THIS frame (0 on warm frames).
    pub blocks_created: u32,
    /// CPU time in `render` (replay + encode + submit), milliseconds.
    pub cpu_ms: f32,
    /// GPU `draw()` invocations encoded (≠ `draws`: a stencil-then-cover
    /// fill is one logical draw but two calls; batched text is many glyphs
    /// in one call).
    pub draw_calls: u32,
    /// Render passes encoded: main-target segments + layer + filter passes.
    pub render_passes: u32,
    /// `set_pipeline` binds — the state changes the GPU actually pays for.
    pub pipeline_switches: u32,
    /// Transient bytes uploaded this frame (Impeller's per-frame HostBuffer
    /// pressure, split by use).
    pub vertex_bytes: u64,
    pub uniform_bytes: u64,
    /// `cpu_ms` split (Impeller's build/raster phases): replay + plan …
    pub plan_ms: f32,
    /// … then pipeline compile + upload + encode + submit.
    pub encode_ms: f32,
}

/// Replays display lists. Owns the frame-scoped and variant caches; holds no
/// content state (retention lives in content-keyed caches, not here).
pub struct RendererCore {
    device: wgpu::Device,
    queue: wgpu::Queue,
    host: HostBuffer,
    pipelines: PipelineCache,
    images: ImageStore,
    pool: TargetPool,
    glyphs: GlyphStore,
    contours: ContourCache,
    ramps: crate::ramps::RampCache,
    rasters: crate::raster::ListRasterCache,
    /// The one linear sampler every filter/composite bind group shares —
    /// created once (a per-frame create is a JS hop on wasm).
    sampler: wgpu::Sampler,
    timer: GpuTimer,
}

impl RendererCore {
    pub fn new(device: wgpu::Device, queue: wgpu::Queue) -> Self {
        let host = HostBuffer::new(&device);
        let pipelines = PipelineCache::new(&device, host.bind_group_layout());
        let images = ImageStore::new(&device, &queue);
        let pool = TargetPool::new(&device);
        let glyphs = GlyphStore::new(&device, &queue);
        let timer = GpuTimer::new(&device, &queue);
        let sampler = crate::plan::linear_sampler(&device);
        Self {
            device,
            queue,
            host,
            pipelines,
            images,
            pool,
            glyphs,
            contours: ContourCache::new(),
            ramps: crate::ramps::RampCache::new(),
            rasters: crate::raster::ListRasterCache::new(),
            sampler,
            timer,
        }
    }

    pub fn images(&mut self) -> &mut ImageStore {
        &mut self.images
    }

    pub fn device(&self) -> &wgpu::Device {
        &self.device
    }

    /// Override the text tier thresholds (see [`crate::TextTiers`]).
    pub fn set_text_tiers(&mut self, tiers: crate::TextTiers) {
        self.glyphs.tiers = tiers;
    }

    /// Blank instead of tofu for unresolved chars, opt-in.
    pub fn set_hide_missing_glyphs(&mut self, hide: bool) {
        self.glyphs.set_hide_missing_glyphs(hide);
    }

    /// The host's gesture switch for SDF rasters.
    pub fn set_text_raster_hold(&mut self, held: bool) {
        self.glyphs.set_text_raster_hold(held);
    }

    /// A camera gesture is in flight: the raster cache prefers reusing
    /// existing textures (at any scale ratio) over refilling — the host
    /// clears this on gesture settle and crisp refills follow.
    pub fn set_raster_hold(&mut self, held: bool) {
        self.rasters.set_hold(held);
    }

    pub fn render(&mut self, dl: &DisplayList, target: &RenderTarget) -> RenderStats {
        #[cfg(feature = "trace")]
        let _span = tracing::info_span!("valo.render", draws = dl.draw_count()).entered();
        let t0 = web_time::Instant::now();
        let blocks_before = self.host.blocks_created;

        self.host.begin_frame();
        let plan = self.plan(dl, target);
        let t_planned = web_time::Instant::now();

        let mut stats = plan.stats;
        self.glyphs.flush_uploads();
        (stats.uniform_bytes, stats.vertex_bytes) = self.upload_and_compile(&plan);
        self.encode_and_submit(&plan, target, &mut stats);
        (stats.glyph_rasters, stats.atlas_gcs, stats.held_rasters) = self.glyphs.frame_counters();
        self.pool.end_frame();
        self.images.end_frame();
        self.contours.end_frame();
        self.glyphs.end_frame();
        self.ramps.end_frame();
        self.rasters.end_frame();

        stats.render_passes = plan.passes.len() as u32;
        stats.blocks_created = (self.host.blocks_created - blocks_before) as u32;
        stats.plan_ms = (t_planned - t0).as_secs_f32() * 1000.0;
        stats.encode_ms = t_planned.elapsed().as_secs_f32() * 1000.0;
        stats.cpu_ms = t0.elapsed().as_secs_f32() * 1000.0;
        stats.gpu_ms = self.timer.latest_ms(&self.device);
        stats
    }

    /// Resource totals on demand (Skia's getResourceCacheUsage tier); each
    /// subsystem reports itself.
    pub fn memory_report(&self) -> crate::MemoryReport {
        crate::MemoryReport {
            images: self.images.report(),
            atlas: self.glyphs.report_atlas(),
            targets: self.pool.report(),
            host_buffer: self.host.report(),
            contours: self.contours.report(),
            glyph_paths: self.glyphs.report_paths(),
            ramps: self.ramps.report(),
            raster_cache: self.rasters.report(),
            wgpu: crate::report::wgpu_counters(&self.device),
        }
    }

    fn plan(&mut self, dl: &DisplayList, target: &RenderTarget) -> FramePlan {
        #[cfg(feature = "trace")]
        let _span = tracing::info_span!("valo.plan").entered();
        // Disjoint field borrows: the planner mutates arenas + pools while
        // reading the pipeline cache's layouts.
        let Self {
            device,
            queue,
            host,
            images,
            pool,
            pipelines,
            glyphs,
            contours,
            ramps,
            rasters,
            sampler,
            ..
        } = self;
        Planner::new(
            device, queue, host, images, pool, pipelines, glyphs, contours, ramps, rasters,
            sampler, target, dl,
        )
        .run(dl)
    }

    fn upload_and_compile(&mut self, plan: &FramePlan) -> (u64, u64) {
        let bytes = self.host.flush(&self.queue);
        for pass in &plan.passes {
            for step in &pass.steps {
                self.pipelines.ensure(&self.device, step.key);
            }
        }
        bytes
    }

    fn encode_and_submit(
        &mut self,
        plan: &FramePlan,
        target: &RenderTarget,
        stats: &mut RenderStats,
    ) {
        #[cfg(feature = "trace")]
        let _span = tracing::info_span!("valo.encode", passes = plan.passes.len()).entered();
        let mut encoder = self
            .device
            .create_command_encoder(&wgpu::CommandEncoderDescriptor {
                label: Some("valo.frame"),
            });
        for (index, pass) in plan.passes.iter().enumerate() {
            self.encode_copies(&mut encoder, pass);
            let timing = self.timer.pass_writes(index, plan.passes.len());
            self.encode_pass(&mut encoder, pass, target, timing, stats);
        }
        self.timer.end_frame(&mut encoder);
        self.queue.submit(std::iter::once(encoder.finish()));
        self.timer.after_submit();
    }

    /// Dst snapshots for advanced blends: land BEFORE the segment that
    /// samples them. Same origin in src and dst — the snapshot shares the
    /// target's coordinates, so only the region under the draw is copied.
    fn encode_copies(&self, encoder: &mut wgpu::CommandEncoder, pass: &PlannedPass) {
        for copy in &pass.pre_copies {
            let origin = wgpu::Origin3d {
                x: copy.origin[0],
                y: copy.origin[1],
                z: 0,
            };
            encoder.copy_texture_to_texture(
                copy_at(&copy.src, origin),
                copy_at(&copy.dst, origin),
                wgpu::Extent3d {
                    width: copy.size[0],
                    height: copy.size[1],
                    depth_or_array_layers: 1,
                },
            );
        }
    }

    fn encode_pass(
        &self,
        encoder: &mut wgpu::CommandEncoder,
        pass: &PlannedPass,
        target: &RenderTarget,
        timing: Option<wgpu::RenderPassTimestampWrites>,
        stats: &mut RenderStats,
    ) {
        let color = match &pass.color {
            PassColor::Main { msaa } => color_attachment(msaa, Some(target.view), pass),
            PassColor::Layer { msaa, resolve } => color_attachment(msaa, Some(resolve), pass),
            PassColor::Filter { view } => color_attachment(view, None, pass),
        };
        let color_attachments = [Some(color)];
        let mut rp = encoder.begin_render_pass(&wgpu::RenderPassDescriptor {
            label: Some("valo.pass"),
            color_attachments: &color_attachments,
            depth_stencil_attachment: pass
                .depth
                .as_ref()
                .map(|depth| depth_attachment(depth, pass.clear_depth, pass.store)),
            timestamp_writes: timing,
            occlusion_query_set: None,
            multiview_mask: None,
        });
        rp.set_stencil_reference(0);
        let mut bound = None;
        for step in &pass.steps {
            if bound != Some(step.key) {
                rp.set_pipeline(self.pipelines.get(&step.key));
                bound = Some(step.key);
                stats.pipeline_switches += 1;
            }
            stats.draw_calls += 1;
            rp.set_bind_group(
                0,
                self.host.bind_group(step.uniforms.block),
                &[step.uniforms.offset],
            );
            if let Some(texture) = &step.texture {
                rp.set_bind_group(1, texture, &[]);
            }
            match step.mesh {
                None => rp.draw(0..6, 0..1),
                Some((slot, vertex_count)) => {
                    let buffer = self.host.vertex_buffer(slot.block);
                    rp.set_vertex_buffer(0, buffer.slice(slot.offset..slot.offset + slot.bytes));
                    rp.draw(0..vertex_count, 0..1);
                }
            }
        }
    }
}

fn copy_at(texture: &wgpu::Texture, origin: wgpu::Origin3d) -> wgpu::TexelCopyTextureInfo<'_> {
    wgpu::TexelCopyTextureInfo {
        texture,
        mip_level: 0,
        origin,
        aspect: wgpu::TextureAspect::All,
    }
}

/// Main/layer passes render ×4 and resolve every segment; MSAA contents are
/// stored only when a later segment resumes the target (the final segment
/// discards — skips the 4× write-out on tiled GPUs). Filter passes render
/// 1-sample straight into `view` (no resolve) and always store.
fn color_attachment<'a>(
    view: &'a wgpu::TextureView,
    resolve: Option<&'a wgpu::TextureView>,
    pass: &PlannedPass,
) -> wgpu::RenderPassColorAttachment<'a> {
    wgpu::RenderPassColorAttachment {
        view,
        depth_slice: None,
        resolve_target: resolve,
        ops: wgpu::Operations {
            load: match pass.clear {
                Some(c) => wgpu::LoadOp::Clear(wgpu::Color {
                    r: (c.r * c.a) as f64,
                    g: (c.g * c.a) as f64,
                    b: (c.b * c.a) as f64,
                    a: c.a as f64,
                }),
                None => wgpu::LoadOp::Load,
            },
            store: store_op(pass.store),
        },
    }
}

/// Depth/stencil persist across a target's segments (clip ceilings survive
/// pass breaks); only the first segment clears, only resumed segments store.
fn depth_attachment(
    view: &wgpu::TextureView,
    clear: bool,
    store: bool,
) -> wgpu::RenderPassDepthStencilAttachment<'_> {
    wgpu::RenderPassDepthStencilAttachment {
        view,
        depth_ops: Some(wgpu::Operations {
            load: if clear {
                wgpu::LoadOp::Clear(0.0)
            } else {
                wgpu::LoadOp::Load
            },
            store: store_op(store),
        }),
        stencil_ops: Some(wgpu::Operations {
            load: if clear {
                wgpu::LoadOp::Clear(0)
            } else {
                wgpu::LoadOp::Load
            },
            store: store_op(store),
        }),
    }
}

fn store_op(store: bool) -> wgpu::StoreOp {
    if store {
        wgpu::StoreOp::Store
    } else {
        wgpu::StoreOp::Discard
    }
}
