//! The frame planner: a `DisplayList` becomes a [`FramePlan`] — an ordered
//! sequence of render passes (main-target segments, layer passes, and the
//! texture copies between them). The encoder replays the plan blindly; every
//! decision — culling, z, layer sizing/elision, pass breaks — is made here.

use std::sync::Arc;
use valo_dl::{
    BlendMode, BlurStyle, ClipOp, ColorFilter, DisplayList, FocalCircle, GlyphPos, Image,
    ImageFilter, MaskBlur, MaskKind, Op, Paint, PaintStyle, Sampling, Shader, SpreadMode, TileMode,
    MAX_GRADIENT_STOPS,
};
use valo_text::{Font, GlyphStroke};

use valo_geometry::{
    dash_contours, local_tolerance, stroke_strip, Color, FillRule, Matrix, Path, Point, Rect,
    Stroke,
};

use crate::contours::ContourCache;
use crate::glyphs::{Coverage, GlyphStore, PageRef, TextTiers};
use crate::host_buffer::{DrawSlot, HostBuffer, VertexSlot, UNIFORM_SIZE};
use crate::images::{ImageStore, IMAGE_FORMAT};
use crate::pipelines::{
    advanced_mode_id, blend_filter_id, blur_style_id, Frag, PipelineCache, PipelineKey,
    PipelineKind, TextMode,
};
use crate::pool::{TargetPool, FILTER_SIZE_BUCKET};
use crate::raster::{FillTarget, ListRasterCache, QuadSource, RasterVerdict};
use crate::renderer::{RenderStats, RenderTarget};

// ── plan structures (consumed by the encoder) ───────────────────────────────

pub(crate) struct FramePlan {
    pub passes: Vec<PlannedPass>,
    pub stats: RenderStats,
}

pub(crate) struct PlannedPass {
    pub color: PassColor,
    /// `None` for filter passes — bare color work, no clips, no StC.
    pub depth: Option<wgpu::TextureView>,
    /// `Some` clears; `None` loads (a resumed segment, or the caller asked
    /// to draw over existing content).
    pub clear: Option<Color>,
    /// First segment of a target clears depth/stencil; resumed ones load
    /// (clip ceilings must survive pass breaks).
    pub clear_depth: bool,
    /// Keep msaa + depth contents at pass end. True only when a later
    /// segment resumes this target (Impeller's
    /// kStoreAndMultisampleResolve vs kMultisampleResolve) — the final
    /// segment discards, which on tiled GPUs skips the 4× tile flush.
    pub store: bool,
    /// Snapshot copies that must complete before this pass runs.
    pub pre_copies: Vec<TextureCopy>,
    pub steps: Vec<Step>,
}

fn replace_msaa(color: &mut PassColor, msaa: &wgpu::TextureView) {
    match color {
        PassColor::Main { msaa: attachment }
        | PassColor::Layer {
            msaa: attachment, ..
        } => *attachment = msaa.clone(),
        PassColor::Filter { .. } => unreachable!("filter passes never open a frame"),
    }
}

pub(crate) enum PassColor {
    /// MSAA scratch resolving into the caller's view.
    Main { msaa: wgpu::TextureView },
    /// A pooled layer: both attachments ours.
    Layer {
        msaa: wgpu::TextureView,
        resolve: wgpu::TextureView,
    },
    /// A gaussian filter pass: 1-sample pooled target, drawn then sampled.
    Filter { view: wgpu::TextureView },
}

/// Region copy at the SAME coordinates in src and dst: the snapshot is
/// target-sized, so `dst_sample`'s uv math needs no remapping — only the
/// pixels the dst-reading draw can actually sample get copied.
pub(crate) struct TextureCopy {
    pub src: wgpu::Texture,
    pub dst: wgpu::Texture,
    pub origin: [u32; 2],
    pub size: [u32; 2],
}

/// One encoded drawing step: a pipeline, one uniform record, an optional
/// group-1 bind group (image / snapshot / blend), and either the built-in
/// unit quad or a transient mesh.
pub(crate) struct Step {
    pub key: PipelineKey,
    pub uniforms: DrawSlot,
    pub texture: Option<wgpu::BindGroup>,
    pub mesh: Option<(VertexSlot, u32)>,
    /// The draw's z — the reorder pass sorts hoisted opaque units by it.
    pub sort_z: f32,
}

// ── uniform record (shaders/solid.wgsl layout contract) ─────────────────────

const PAYLOAD_RECT: usize = 0;
const PAYLOAD_GEOM: usize = 1;
const PAYLOAD_MISC: usize = 2;
const PAYLOAD_OFFSETS: usize = 3; // ..5: 8 stop offsets
const PAYLOAD_RADII: usize = 3; // rrect-blur corner radii (no gradient there)
const PAYLOAD_DECAL: usize = 3; // per-axis image decal flags (no gradient there)
const PAYLOAD_COLORS: usize = 5; // ..13: 8 premultiplied stop colors
const PAYLOAD_LOCAL: usize = 13; // ..15: inverse gradient local matrix
const PAYLOAD_CONICAL: usize = 15; // two-point conical case + constants
const PAYLOAD_CONICAL_FLAGS: usize = 16; // (swapped, focal on circle, well behaved)
const PAYLOAD_COLOR_MATRIX: usize = 17; // ..22: 4 matrix rows + the translation column;
                                        // doubles as the blend filter's source colour

struct UniformRecord {
    bytes: [u8; UNIFORM_SIZE as usize],
}

impl UniformRecord {
    fn new(mvp: [f32; 16], color: [f32; 4]) -> Self {
        let mut bytes = [0u8; UNIFORM_SIZE as usize];
        bytes[..64].copy_from_slice(bytemuck::cast_slice(&mvp));
        bytes[64..80].copy_from_slice(bytemuck::cast_slice(&color));
        Self { bytes }
    }

    fn set_payload(&mut self, index: usize, v: [f32; 4]) {
        let at = 80 + index * 16;
        self.bytes[at..at + 16].copy_from_slice(bytemuck::cast_slice(&v));
    }

    fn set_local_rect(&mut self, r: &Rect) {
        self.set_payload(PAYLOAD_RECT, [r.x, r.y, r.width, r.height]);
    }
}

// ── planner state ───────────────────────────────────────────────────────────

/// One open render target being planned (main, or a save layer). Segments
/// (pass breaks) accumulate in `steps` and emit on break/close.
struct PassFrame {
    color: PassColor,
    depth: wgpu::TextureView,
    /// Where dst snapshots copy FROM (this target's resolved image).
    src_texture: wgpu::Texture,
    size: [u32; 2],
    clear: Option<Color>,
    first_segment_emitted: bool,
    /// Index of this frame's most recent emitted segment — backpatched to
    /// `store` when a later segment resumes the target.
    last_pass: Option<usize>,
    steps: Vec<Step>,
    pre_copies: Vec<TextureCopy>,
    /// Cull rect for ops planned into this frame, in the coords op bounds
    /// map into (the PARENT pass's space for layers).
    cull_rect: Rect,
    z_denom: f32,
    /// The parent's suspended slot offset, restored when this frame closes.
    outer_slot_offset: i64,
    /// The parent's suspended elision stack: group alpha applies ONCE, at
    /// this frame's composite — never to draws inside it.
    outer_elisions: Vec<f32>,
    /// Origin shift for draws into this frame (layer-local pixels): the
    /// layer's rect origin in parent coords.
    origin: Point,
    /// Attachments are tile-only; flips to false (with a texture swap) on
    /// the first resume — stored segments need real memory.
    transient: bool,
    /// Set for layer frames: everything the composite needs at close.
    layer: Option<LayerInfo>,
}

struct LayerInfo {
    /// The layer's region in the PARENT pass's coordinates.
    rect: Rect,
    paint: Paint,
    /// Set = mask layer: composite converts to coverage (DstIn).
    mask_composite: Option<MaskKind>,
    /// Parent-space z of the composite draw.
    composite_z: f32,
    resolve: wgpu::TextureView,
    /// What runs over the texture before it composites.
    effects: LayerEffects,
}

/// A layer's post-effects. Impeller applies the two in OPPOSITE orders
/// depending on what the layer is, and so does valo:
///
/// - a draw's own effect layer follows `Paint::WithFilters` — colour filter
///   first, then the blur spreads the filtered pixels;
/// - a `save_layer` subpass follows `Paint::WithFiltersForSubpassTarget` —
///   image filter first, colour filter over the blurred result.
///
/// The difference shows wherever the blur produces fractional alpha, since a
/// matrix that translates or clamps is not commutative with it.
#[derive(Clone, Default)]
struct LayerEffects {
    color_filter: Option<ColorFilter>,
    image_filter: Option<ImageFilter>,
    /// σ is in DEVICE px by the time it lands here. Non-Normal styles add a
    /// combine pass merging the blur with the sharp layer.
    blur: Option<MaskBlur>,
    /// Set for a recorded `save_layer`; clear for the implicit layers a draw
    /// opens for its own effects.
    subpass: bool,
    /// The effect transform's 2×2 basis `[a, b, c, d]`. The whole basis is
    /// kept, not its two axis lengths: an image filter's σ is a VECTOR, and
    /// under rotation its axes have to move with the matrix.
    image_basis: [f32; 4],
}

impl LayerEffects {
    /// The effects a paint asks of its layer. The transform converts local
    /// blur axes into device-pixel sigma, as Impeller's effect transform does.
    fn of(paint: &Paint, mask_scale: f32, image_transform: &Matrix, subpass: bool) -> Self {
        let [a, b, c, d, ..] = image_transform.to_affine();
        Self {
            color_filter: paint.color_filter,
            image_filter: paint.effective_image_filter().cloned(),
            blur: paint.mask_blur.map(|mask| MaskBlur {
                sigma: (mask.sigma * mask_scale).max(0.05),
                style: mask.style,
            }),
            subpass,
            image_basis: [a, b, c, d],
        }
    }

    fn is_empty(&self) -> bool {
        self.color_filter.is_none() && self.image_filter.is_none() && self.blur.is_none()
    }
}

/// One shared backdrop key's blur, registered by the first tile replayed;
/// later same-key tiles composite it without another pass break.
struct SharedBlur {
    view: wgpu::TextureView,
    /// The blurred region, absolute replay coords.
    region: Rect,
    uv_max: [f32; 2],
    /// Device σ the blur ran at — a tile whose σ differs (a transform can
    /// split record-time-equal σs) blurs independently instead
    /// (Impeller's all_filters_equal fallback).
    sigma: f32,
}

/// A finished filter chain: sample `view` up to `uv_max` (the used corner of
/// the bucketed, possibly downsampled target).
#[derive(Clone)]
struct FilteredTexture {
    view: wgpu::TextureView,
    uv_max: [f32; 2],
    /// The target's own dimensions — bucketed, so bigger than the region
    /// actually used. A pass that reads this texture must scale its uv by
    /// THESE, not by the layer's size.
    size: [u32; 2],
}

impl FilteredTexture {
    fn source(view: wgpu::TextureView, size: [u32; 2], whole: &Rect) -> Self {
        Self {
            view,
            uv_max: [whole.width / size[0] as f32, whole.height / size[1] as f32],
            size,
        }
    }
}

struct ColorFilterTarget {
    view: wgpu::TextureView,
    size: [u32; 2],
    format: wgpu::TextureFormat,
}

#[derive(Clone, Copy)]
enum EncodedColorFilter {
    Matrix,
    Blend,
}

/// What kind of scope each recorded Save/SaveLayer opened — replay's Restore
/// dispatch (local to one list; the builder guarantees per-list balance).
enum ScopeKind {
    Plain,
    Elided,
    Layer,
}

pub(crate) struct Planner<'a> {
    device: &'a wgpu::Device,
    queue: &'a wgpu::Queue,
    host: &'a mut HostBuffer,
    images: &'a mut ImageStore,
    pool: &'a mut TargetPool,
    pipelines: &'a PipelineCache,
    glyphs: &'a mut GlyphStore,
    contours: &'a mut ContourCache,
    ramps: &'a mut crate::ramps::RampCache,
    format: wgpu::TextureFormat,
    sampler: wgpu::Sampler,
    frames: Vec<PassFrame>,
    passes: Vec<PlannedPass>,
    /// Opacity elision (Impeller's peephole): an elided layer's children
    /// render in the parent pass at their OWN slots — the group alpha rides
    /// their tint. Compounds like Impeller's `distributed_opacity`; depth
    /// never changes on elision, which is what makes it safe.
    elisions: Vec<f32>,
    /// Live shared backdrop blurs for the list replay we're INSIDE — set
    /// aside and restored around nested replays (like the elision stack),
    /// so a retained list drawn twice never reuses the first drawing's
    /// blur region.
    shared_blurs: rustc_hash::FxHashMap<u64, SharedBlur>,
    /// Maps the current list's record-time slots into the current frame's
    /// depth space: nested lists ADD their embed offset, layer frames
    /// SUBTRACT their base_slot (all slots live on one global line).
    slot_offset: i64,
    rasters: &'a mut ListRasterCache,
    /// Inside a fill's replay, nested hints render inline — the ancestor's
    /// raster is the caching unit.
    filling_raster: bool,
    stats: RenderStats,
}

impl<'a> Planner<'a> {
    #[allow(clippy::too_many_arguments)] // one-shot constructor wiring the renderer's parts
    pub fn new(
        device: &'a wgpu::Device,
        queue: &'a wgpu::Queue,
        host: &'a mut HostBuffer,
        images: &'a mut ImageStore,
        pool: &'a mut TargetPool,
        pipelines: &'a PipelineCache,
        glyphs: &'a mut GlyphStore,
        contours: &'a mut ContourCache,
        ramps: &'a mut crate::ramps::RampCache,
        rasters: &'a mut ListRasterCache,
        sampler: &wgpu::Sampler,
        target: &RenderTarget,
        dl: &DisplayList,
    ) -> Self {
        // A load-existing target (clear: None) must keep its msaa contents
        // across FRAMES — only cleared targets can go tile-only.
        let transient = target.clear.is_some();
        let scratch = pool.main_scratch(target.size, target.format, transient);
        let main = PassFrame {
            color: PassColor::Main { msaa: scratch.msaa },
            depth: scratch.depth,
            src_texture: target.texture.clone(),
            size: target.size,
            clear: target.clear,
            first_segment_emitted: false,
            last_pass: None,
            steps: Vec::with_capacity(dl.draw_count() as usize * 2),
            pre_copies: Vec::new(),
            cull_rect: Rect::new(0.0, 0.0, target.size[0] as f32, target.size[1] as f32),
            z_denom: (dl.depth_slots() + 1) as f32,
            outer_slot_offset: 0,
            outer_elisions: Vec::new(),
            origin: Point::ZERO,
            transient,
            layer: None,
        };
        Self {
            device,
            queue,
            host,
            images,
            pool,
            pipelines,
            glyphs,
            contours,
            ramps,
            format: target.format,
            sampler: sampler.clone(),
            frames: vec![main],
            passes: Vec::new(),
            elisions: Vec::new(),
            shared_blurs: rustc_hash::FxHashMap::default(),
            slot_offset: 0,
            rasters,
            filling_raster: false,
            stats: RenderStats::default(),
        }
    }

    pub fn run(mut self, dl: &DisplayList) -> FramePlan {
        let mut stack = vec![Matrix::IDENTITY];
        self.replay_list(dl, &mut stack, Matrix::IDENTITY);
        self.emit_segment();
        FramePlan {
            passes: self.passes,
            stats: self.stats,
        }
    }

    // ── replay ──────────────────────────────────────────────────────────────

    /// Walk one list. `stack` is the live transform stack (local → current
    /// pass coords, PARENT coords for layer children — the layer's origin
    /// shift is applied at MVP time); `base` maps this list's root space
    /// into the same coords (for the record-time bounds).
    fn replay_list(&mut self, dl: &DisplayList, stack: &mut Vec<Matrix>, base: Matrix) {
        let ops = dl.ops();
        let mut scopes: Vec<ScopeKind> = Vec::new();
        let mut i = 0;
        while i < ops.len() {
            self.stats.ops += 1;
            match &ops[i] {
                Op::Save => {
                    stack.push(*stack.last().unwrap());
                    scopes.push(ScopeKind::Plain);
                }
                Op::SaveLayer {
                    paint,
                    mask_composite,
                    scope_bounds,
                    base_slot,
                    composite_slot,
                    can_elide,
                } => {
                    let composite = Composite {
                        paint: paint.clone(),
                        mask: *mask_composite,
                    };
                    match self.open_layer(
                        &base,
                        stack.last().unwrap(),
                        composite,
                        LayerScope {
                            bounds: scope_bounds,
                            base_slot: *base_slot,
                            composite_slot: *composite_slot,
                            can_elide: *can_elide,
                        },
                    ) {
                        Opened::Skip => {
                            i = skip_scope(ops, i) + 1;
                            continue;
                        }
                        opened => {
                            stack.push(*stack.last().unwrap());
                            scopes.push(match opened {
                                Opened::Elided => ScopeKind::Elided,
                                _ => ScopeKind::Layer,
                            });
                        }
                    }
                }
                Op::Restore => {
                    stack.pop();
                    match scopes.pop() {
                        Some(ScopeKind::Plain) | None => {}
                        Some(ScopeKind::Elided) => {
                            self.elisions.pop();
                        }
                        Some(ScopeKind::Layer) => self.close_layer(),
                    }
                }
                Op::Transform(t) => {
                    let top = stack.last_mut().unwrap();
                    *top = top.then(t);
                }
                Op::DrawRect {
                    rect,
                    paint,
                    bounds,
                    slot,
                } => {
                    if !self.culled(&base, bounds, 1) {
                        let z = self.slot_z(*slot);
                        self.plan_rect(rect, paint, stack.last().unwrap(), z);
                    }
                }
                Op::DrawPath {
                    path,
                    fill_rule,
                    paint,
                    bounds,
                    slot,
                } => {
                    if !self.culled(&base, bounds, 1) {
                        let z = self.slot_z(*slot);
                        self.plan_path_fill(path, *fill_rule, paint, stack.last().unwrap(), z);
                    }
                }
                Op::DrawImage {
                    image,
                    src,
                    dst,
                    sampling,
                    paint,
                    bounds,
                    slot,
                } => {
                    if !self.culled(&base, bounds, 1) {
                        let z = self.slot_z(*slot);
                        let current = *stack.last().unwrap();
                        self.plan_image(image, src, dst, *sampling, paint, &current, z);
                    }
                }
                Op::RRectBlur {
                    rect,
                    radii,
                    paint,
                    bounds,
                    slot,
                } => {
                    if !self.culled(&base, bounds, 1) {
                        let z = self.slot_z(*slot);
                        let current = *stack.last().unwrap();
                        self.plan_rrect_blur(rect, *radii, paint, &current, z);
                    }
                }
                Op::BackdropBlur {
                    rect: _,
                    sigma,
                    shared_key,
                    bounds,
                    slot,
                } => {
                    if !self.culled(&base, bounds, 1) {
                        let z = self.slot_z(*slot);
                        let current = *stack.last().unwrap();
                        self.plan_backdrop(dl, *sigma, *shared_key, &base, &current, bounds, z);
                    }
                }
                Op::GlyphRun {
                    font,
                    size,
                    paint,
                    glyphs,
                    bounds,
                    slot,
                } => {
                    if !self.culled(&base, bounds, 1) {
                        let z = self.slot_z(*slot);
                        let current = *stack.last().unwrap();
                        let device = base.map_rect(bounds);
                        self.plan_glyph_run(font, *size, paint, glyphs, device, &current, z);
                    }
                }
                Op::ClipPath {
                    path,
                    fill_rule,
                    op,
                    expiry_slot,
                    ..
                } => {
                    // Never bounds-culled: an Intersect ceiling covers the
                    // whole target MINUS the shape.
                    let z = self.slot_z(*expiry_slot);
                    self.plan_clip(path, *fill_rule, *op, stack.last().unwrap(), z);
                }
                Op::DrawDisplayList {
                    list,
                    bounds,
                    base_slot,
                    cache,
                } => {
                    if !self.culled(&base, bounds, list.draw_count()) {
                        let embed = *stack.last().unwrap();
                        if *cache && !self.filling_raster {
                            self.embed_cached_list(list, *base_slot, &embed);
                        } else {
                            self.replay_embedded(list, *base_slot, &embed);
                        }
                    }
                }
            }
            i += 1;
        }
    }

    /// Inline replay of an embedded list (the unhinted path, and every
    /// fallback of the hinted one).
    fn replay_embedded(&mut self, list: &Arc<DisplayList>, base_slot: u32, embed: &Matrix) {
        let saved_offset = self.slot_offset;
        let saved_blurs = std::mem::take(&mut self.shared_blurs);
        self.slot_offset += base_slot as i64;
        let mut child_stack = vec![*embed];
        self.replay_list(list, &mut child_stack, *embed);
        self.slot_offset = saved_offset;
        self.shared_blurs = saved_blurs;
    }

    /// A hinted embed: sample the cached raster as one quad, or
    /// fall back to inline replay — scheduling a fill when the cache asks
    /// (the fill renders as an extra pass; its quad takes over NEXT frame,
    /// so a fill never changes the pixels the user is looking at).
    fn embed_cached_list(&mut self, list: &Arc<DisplayList>, base_slot: u32, embed: &Matrix) {
        // The composite quad is axis-aligned in pass coords: rotated or
        // skewed embeds replay inline (flutter skips integral snapping
        // under complex transforms for the same reason — flutter#41654).
        let [_, shear_b, shear_c, ..] = embed.to_affine();
        if shear_b != 0.0 || shear_c != 0.0 || !embed.is_affine() {
            return self.replay_embedded(list, base_slot, embed);
        }
        let verdict = self.rasters.resolve(
            self.device,
            self.format,
            list,
            embed.max_scale(),
            self.device.limits().max_texture_dimension_2d,
        );
        match verdict {
            RasterVerdict::Quad(source) => self.plan_raster_quad(&source, embed, base_slot),
            RasterVerdict::Fill(target) => {
                let source = target.quad_source();
                self.plan_one_raster_fill(list, target);
                self.plan_raster_quad(&source, embed, base_slot);
            }
            RasterVerdict::Inline => self.replay_embedded(list, base_slot, embed),
        }
    }

    /// One sampled quad standing in for a whole cached sub-list, composited
    /// like a layer (premultiplied SrcOver). At (near-)exact scale the
    /// origin snaps to integral device px and the dest takes the texture's
    /// own integer size — texel-perfect against inline replay (flutter's
    /// GetIntegralTransCTM discipline).
    fn plan_raster_quad(&mut self, source: &QuadSource, embed: &Matrix, base_slot: u32) {
        self.stats.raster_quads += 1;
        let mapped = embed.map_rect(&source.content_bounds);
        let ratio = embed.max_scale() / source.content_scale.max(1e-6);
        let exact = (ratio - 1.0).abs() < 1e-3;
        let texture_extent = if exact {
            [source.size[0] as f32, source.size[1] as f32]
        } else {
            [source.size[0] as f32 * ratio, source.size[1] as f32 * ratio]
        };
        let dest = if exact {
            Rect::new(
                mapped.x.round(),
                mapped.y.round(),
                texture_extent[0],
                texture_extent[1],
            )
        } else {
            mapped
        };
        let z = self.slot_z(base_slot);
        let mut record = self.quad_record(&dest, [1.0, 1.0, 1.0, 1.0], z);
        // UVs map dest coords onto the FULL texture (ceil-sized past the
        // content, like layer textures — same convention as composites).
        let sample = Rect::new(dest.x, dest.y, texture_extent[0], texture_extent[1]);
        record.set_payload(PAYLOAD_GEOM, full_rect_uv(&sample));
        let bind = self.texture_bind(&source.view);
        self.push_step(
            PipelineKind::Draw(Frag::Image),
            BlendMode::SrcOver,
            record,
            Some(bind),
            None,
            z,
        );
    }

    /// Render one fill into its persistent texture, mid-walk — the pass
    /// emits BEFORE the current segment (exactly how every save-layer
    /// renders before the composite that samples it), so the quad drawn
    /// right after this samples pixels already scheduled this frame.
    fn plan_one_raster_fill(&mut self, list: &Arc<DisplayList>, target: FillTarget) {
        self.stats.raster_fills += 1;
        self.filling_raster = true;
        let attachments = self.pool.take_raster_attachments(target.size, self.format);
        // p_texture = scale · (p_list − origin): translate applies first.
        let base = Matrix::scale(target.content_scale, target.content_scale).then(
            &Matrix::translation(-target.content_bounds.x, -target.content_bounds.y),
        );
        self.frames.push(PassFrame {
            color: PassColor::Layer {
                msaa: attachments.msaa,
                resolve: target.view.clone(),
            },
            depth: attachments.depth,
            src_texture: target.texture.clone(),
            size: target.size,
            clear: Some(Color::TRANSPARENT),
            first_segment_emitted: false,
            last_pass: None,
            steps: Vec::new(),
            pre_copies: Vec::new(),
            cull_rect: Rect::new(0.0, 0.0, target.size[0] as f32, target.size[1] as f32),
            z_denom: (list.depth_slots() + 1) as f32,
            outer_slot_offset: std::mem::replace(&mut self.slot_offset, 0),
            outer_elisions: std::mem::take(&mut self.elisions),
            origin: Point::ZERO,
            transient: true,
            layer: None,
        });
        let saved_blurs = std::mem::take(&mut self.shared_blurs);
        let mut stack = vec![base];
        self.replay_list(list, &mut stack, base);
        self.emit_segment();
        let frame = self.frames.pop().expect("fill frame present");
        self.slot_offset = frame.outer_slot_offset;
        self.elisions = frame.outer_elisions;
        self.shared_blurs = saved_blurs;
        self.filling_raster = false;
    }

    fn culled(&mut self, base: &Matrix, bounds: &Rect, draws: u32) -> bool {
        let visible = base.map_rect(bounds).intersects(&self.frame().cull_rect);
        if !visible {
            self.stats.culled += draws;
        }
        !visible
    }

    // ── layers ──────────────────────────────────────────────────────────────

    fn open_layer(
        &mut self,
        base: &Matrix,
        effect_transform: &Matrix,
        composite: Composite,
        scope: LayerScope<'_>,
    ) -> Opened {
        let composite_z = self.slot_z(scope.composite_slot);
        let Some(rect) = self.layer_rect(base, scope.bounds) else {
            if composite.mask.is_some() {
                // A culled/empty MASK isn't "nothing to draw" — it's
                // coverage 0 everywhere: the enclosing layer goes blank.
                self.erase_frame_alpha(composite_z);
            }
            return Opened::Skip;
        };
        if scope.can_elide {
            self.stats.layers_elided += 1;
            self.push_elision(composite.paint.color.a);
            return Opened::Elided;
        }
        self.stats.layers_rendered += 1;
        // The layer paint's σ is local, scaled to device here — same
        // convention as plan_via_effect_layer.
        let effects = LayerEffects::of(&composite.paint, base.max_scale(), effect_transform, true);
        // span + 1 = the composite's distance from the scope's base.
        self.push_layer_frame_rebased(
            rect,
            (scope.composite_slot - scope.base_slot) as f32,
            composite,
            composite_z,
            effects,
            scope.base_slot,
        );
        Opened::Layer
    }

    fn close_layer(&mut self) {
        self.emit_segment();
        let frame = self.frames.pop().expect("layer frame present");
        let info = frame.layer.expect("close_layer only on layer frames");
        self.slot_offset = frame.outer_slot_offset;
        self.elisions = frame.outer_elisions;
        self.composite_layer(&info);
    }

    /// The layer's region in parent coords: recorded scope bounds mapped by
    /// the list base, intersected with what's visible.
    fn layer_rect(&mut self, base: &Matrix, scope_bounds: &Rect) -> Option<Rect> {
        if scope_bounds.is_empty() {
            return None;
        }
        base.map_rect(scope_bounds)
            .intersect(&self.frame().cull_rect)
    }

    /// `rect` is in ABSOLUTE replay coords (the transform stack is never
    /// rebased) — it doubles as the children's cull rect and, minus its
    /// origin, the layer's pixel space.
    fn push_layer_frame(
        &mut self,
        rect: Rect,
        z_denom: f32,
        paint: Paint,
        composite_z: f32,
        effects: LayerEffects,
    ) {
        let composite = Composite { paint, mask: None };
        self.push_layer_frame_rebased(rect, z_denom, composite, composite_z, effects, 0);
    }

    /// `base_slot` rebases recorded slots into this frame's depth space
    /// (children were numbered on the parent-continuing line). Implicit
    /// desugar layers pass 0 — their draws use explicit z, never slots.
    fn push_layer_frame_rebased(
        &mut self,
        rect: Rect,
        z_denom: f32,
        composite: Composite,
        composite_z: f32,
        effects: LayerEffects,
        base_slot: u32,
    ) {
        let Composite { mut paint, mask } = composite;
        // Group alpha from an enclosing elided scope lands ONCE, here.
        paint.color.a *= self.elision_alpha();
        let size = layer_texture_size(&rect);
        let target = self.pool.take_layer(size, self.format, true);
        self.frames.push(PassFrame {
            color: PassColor::Layer {
                msaa: target.msaa,
                resolve: target.resolve.clone(),
            },
            depth: target.depth,
            src_texture: target.resolve_texture,
            size,
            clear: Some(Color::TRANSPARENT),
            first_segment_emitted: false,
            last_pass: None,
            steps: Vec::new(),
            pre_copies: Vec::new(),
            cull_rect: rect,
            z_denom,
            outer_slot_offset: std::mem::replace(&mut self.slot_offset, -(base_slot as i64)),
            outer_elisions: std::mem::take(&mut self.elisions),
            origin: Point::new(rect.x, rect.y),
            transient: true,
            layer: Some(LayerInfo {
                rect,
                paint,
                mask_composite: mask,
                composite_z,
                resolve: target.resolve,
                effects,
            }),
        });
    }

    fn push_elision(&mut self, alpha: f32) {
        let combined = alpha * self.elisions.last().copied().unwrap_or(1.0);
        self.elisions.push(combined);
    }

    /// Draw the finished layer texture into the parent: a plain
    /// alpha/blend composite is one textured quad; an advanced blend runs
    /// the snapshot dance in the parent. A blur layer's texture is blurred
    /// first (the filter passes land between the parent's segments).
    fn composite_layer(&mut self, info: &LayerInfo) {
        if let Some(kind) = info.mask_composite {
            return self.composite_mask_layer(info, kind);
        }
        let (view, uv) = self.composite_source(info);
        let tint = alpha_tint(info.paint.color.a);
        if info.paint.blend_mode.is_pipeline_blendable() {
            let mut record = self.quad_record(&info.rect, tint, info.composite_z);
            record.set_payload(PAYLOAD_GEOM, uv);
            let bind = self.texture_bind(&view);
            self.push_step(
                PipelineKind::Draw(Frag::Image),
                info.paint.blend_mode,
                record,
                Some(bind),
                None,
                info.composite_z,
            );
        } else {
            let snapshot = self.break_pass(&info.rect);
            let mut record = self.quad_record(&info.rect, tint, info.composite_z);
            record.set_payload(PAYLOAD_GEOM, uv);
            self.set_blend_misc(&mut record, info.paint.blend_mode);
            let bind = self.blend_bind(&snapshot, &view);
            self.push_step(
                PipelineKind::Draw(Frag::BlendTexture),
                BlendMode::SrcOver,
                record,
                Some(bind),
                None,
                info.composite_z,
            );
        }
    }

    /// What the composite samples: the layer texture as-is, its blur, or —
    /// for styled masks — the blur COMBINED with the sharp layer. UVs always
    /// divide by the INTEGER texture extent — the layer's rect is fractional
    /// but its texture is ceil-sized, and mixing the two stretches content
    /// by up to a texel at the right/bottom edges.
    fn composite_source(&mut self, info: &LayerInfo) -> (wgpu::TextureView, [f32; 4]) {
        let size = layer_texture_size(&info.rect);
        let sample = Rect::new(info.rect.x, info.rect.y, size[0] as f32, size[1] as f32);
        if info.effects.is_empty() {
            return (info.resolve.clone(), full_rect_uv(&sample));
        }
        let whole = Rect::new(0.0, 0.0, size[0] as f32, size[1] as f32);
        let filtered = if info.effects.subpass {
            self.blur_then_recolour(info, size, &whole)
        } else {
            self.recolour_then_blur(info, size, &whole)
        };
        (filtered.view, region_uv(&sample, filtered.uv_max))
    }

    /// A draw's own effect layer: the colour filter runs on the shape's
    /// pixels and the blur spreads the filtered result. A styled blur
    /// combines against the filtered layer, not the raw one.
    fn recolour_then_blur(
        &mut self,
        info: &LayerInfo,
        size: [u32; 2],
        whole: &Rect,
    ) -> FilteredTexture {
        if info.effects.image_filter.is_none() {
            return self.recolour_then_mask_blur(info, size, whole);
        }
        let mut output = FilteredTexture::source(info.resolve.clone(), size, whole);
        if let Some(filter) = info.effects.color_filter {
            output = self.push_color_filter_input(&output, whole, filter);
        }
        if let Some(filter) = &info.effects.image_filter {
            output = self.push_image_filter(&output, whole, filter, info.effects.image_basis);
        }
        if let Some(mask) = info.effects.blur {
            output = self.blur_filtered_layer(&output, whole, mask);
        }
        output
    }

    /// A `save_layer` subpass: the blur runs first and the colour filter
    /// recolours the blurred result, so a translating or clamping matrix acts
    /// on the halo's fractional alpha the way Flutter's does.
    fn blur_then_recolour(
        &mut self,
        info: &LayerInfo,
        size: [u32; 2],
        whole: &Rect,
    ) -> FilteredTexture {
        if info.effects.image_filter.is_none() {
            return self.mask_blur_then_recolour(info, size, whole);
        }
        let mut output = FilteredTexture::source(info.resolve.clone(), size, whole);
        if let Some(filter) = &info.effects.image_filter {
            output = self.push_image_filter(&output, whole, filter, info.effects.image_basis);
        }
        if let Some(mask) = info.effects.blur {
            output = self.blur_filtered_layer(&output, whole, mask);
        }
        if let Some(filter) = info.effects.color_filter {
            output = self.push_color_filter_input(&output, whole, filter);
        }
        output
    }

    fn recolour_then_mask_blur(
        &mut self,
        info: &LayerInfo,
        size: [u32; 2],
        whole: &Rect,
    ) -> FilteredTexture {
        let recoloured = info
            .effects
            .color_filter
            .map(|filter| self.push_color_filter(&info.resolve, size, whole, filter));
        let (sharp, sharp_size) = recoloured.as_ref().map_or_else(
            || (info.resolve.clone(), size),
            |output| (output.view.clone(), output.size),
        );
        match info.effects.blur {
            None => recoloured.expect("empty effects returned early"),
            Some(mask) => self.blur_layer(&sharp, sharp_size, whole, mask),
        }
    }

    fn mask_blur_then_recolour(
        &mut self,
        info: &LayerInfo,
        size: [u32; 2],
        whole: &Rect,
    ) -> FilteredTexture {
        let blurred = info
            .effects
            .blur
            .map(|mask| self.blur_layer(&info.resolve, size, whole, mask));
        let Some(filter) = info.effects.color_filter else {
            return blurred.expect("empty effects returned early");
        };
        match blurred {
            // A downsampled blur's output is smaller than the layer it stands
            // for. Only the `_input` helper reads it at its own resolution;
            // the raw one takes its source for full semantic size and would
            // shove the halo into the layer's top-left corner.
            Some(blurred) => self.push_color_filter_input(&blurred, whole, filter),
            None => self.push_color_filter(&info.resolve, size, whole, filter),
        }
    }

    fn blur_layer(
        &mut self,
        source: &wgpu::TextureView,
        size: [u32; 2],
        whole: &Rect,
        mask: MaskBlur,
    ) -> FilteredTexture {
        let blurred = self.plan_blur(source, size, whole, mask.sigma, Vec::new());
        match mask.style {
            BlurStyle::Normal => blurred,
            style => self.push_mask_combine(&blurred, source, whole, style),
        }
    }

    fn blur_filtered_layer(
        &mut self,
        source: &FilteredTexture,
        whole: &Rect,
        mask: MaskBlur,
    ) -> FilteredTexture {
        let blurred = self.plan_blur_input(source, whole, mask.sigma, mask.sigma);
        match mask.style {
            BlurStyle::Normal => blurred,
            style => {
                let sharp = self.materialize_filter_input(source, whole);
                self.push_mask_combine(&blurred, &sharp.view, whole, style)
            }
        }
    }

    /// Composite a MASK layer: its texture becomes coverage
    /// (luminance of premultiplied pixels IS luma×alpha, so one dot; alpha
    /// kind reads alpha) and multiplies the enclosing layer via DstIn. The
    /// quad spans the WHOLE enclosing frame — outside the mask's rect the
    /// fragment forces coverage 0, which is what erases unmasked content.
    fn composite_mask_layer(&mut self, info: &LayerInfo, kind: MaskKind) {
        let extent = self.frame().cull_rect;
        let size = layer_texture_size(&info.rect);
        let mut record =
            self.quad_record(&extent, alpha_tint(info.paint.color.a), info.composite_z);
        let (w, h) = (size[0] as f32, size[1] as f32);
        record.set_payload(
            PAYLOAD_GEOM,
            [1.0 / w, 1.0 / h, -info.rect.x / w, -info.rect.y / h],
        );
        let luma = match kind {
            MaskKind::Luminance => 1.0,
            MaskKind::Alpha => 0.0,
        };
        record.set_payload(PAYLOAD_MISC, [luma, 0.0, 0.0, 0.0]);
        let bind = self.texture_bind(&info.resolve);
        self.push_step(
            PipelineKind::Draw(Frag::MaskComposite),
            BlendMode::DstIn,
            record,
            Some(bind),
            None,
            info.composite_z,
        );
    }

    /// DstIn with zero source alpha over the whole frame — the "mask never
    /// rendered" composite (coverage 0 everywhere).
    fn erase_frame_alpha(&mut self, z: f32) {
        let extent = self.frame().cull_rect;
        let record = self.quad_record(&extent, [0.0; 4], z);
        self.push_step(
            PipelineKind::Draw(Frag::Solid),
            BlendMode::DstIn,
            record,
            None,
            None,
            z,
        );
    }

    // ── pass breaks (advanced blends) ───────────────────────────────────────

    /// End the current segment and schedule a dst snapshot: the NEXT segment
    /// starts by copying the target's resolved contents under `coverage`
    /// (the dst-reading draw's device bounds — the only pixels it can sample).
    fn break_pass(&mut self, coverage: &Rect) -> wgpu::TextureView {
        self.stats.snapshots += 1;
        self.emit_segment();
        let (size, origin, src) = {
            let frame = self.frame();
            (frame.size, frame.origin, frame.src_texture.clone())
        };
        let snapshot = self.pool.take_snapshot(size, self.format);
        if let Some((origin, extent)) = snapshot_region(coverage, origin, size) {
            self.frame_mut().pre_copies.push(TextureCopy {
                src,
                dst: snapshot.texture.clone(),
                origin,
                size: extent,
            });
        }
        snapshot.view
    }

    fn emit_segment(&mut self) {
        {
            let frame = self.frame();
            let first = !frame.first_segment_emitted;
            if frame.steps.is_empty() && frame.pre_copies.is_empty() && !first {
                return;
            }
        }
        if self.frame().last_pass.is_some() && self.frame().transient {
            // This emit resumes the target: stored segments need real
            // memory, so tile-only attachments swap out first.
            self.swap_to_persistent_attachments();
        }
        let index = self.passes.len();
        let frame = self.frame_mut();
        let first = !frame.first_segment_emitted;
        frame.first_segment_emitted = true;
        let resumed = frame.last_pass.replace(index);
        let color = match &frame.color {
            PassColor::Main { msaa } => PassColor::Main { msaa: msaa.clone() },
            PassColor::Layer { msaa, resolve } => PassColor::Layer {
                msaa: msaa.clone(),
                resolve: resolve.clone(),
            },
            PassColor::Filter { .. } => unreachable!("filter passes never open a frame"),
        };
        let mut hoisted = 0;
        let steps = reorder_segment(std::mem::take(&mut frame.steps), &mut hoisted);
        let pass = PlannedPass {
            color,
            depth: Some(frame.depth.clone()),
            clear: if first { frame.clear } else { None },
            clear_depth: first,
            // A load-existing target (clear: None) keeps the old always-store
            // behavior: its next frame Loads the msaa attachment, which a
            // proper un-resolve would make unnecessary.
            store: frame.clear.is_none(),
            pre_copies: std::mem::take(&mut frame.pre_copies),
            steps,
        };
        self.stats.opaque_reordered += hoisted;
        if let Some(prev) = resumed {
            self.passes[prev].store = true;
        }
        self.passes.push(pass);
    }

    /// A target that turned out to need stores (a dst-reading break splits
    /// it into segments) cannot render into tile-only attachments — swap
    /// in a persistent msaa + depth pair, both for the segments to come
    /// and for the one already emitted (planning-time swap: nothing has
    /// rendered yet). The resolve — where snapshots copy from — stays.
    fn swap_to_persistent_attachments(&mut self) {
        let size = self.frame().size;
        let scratch = self.pool.main_scratch(size, self.format, false);
        let previous = self.frame().last_pass;
        let frame = self.frame_mut();
        frame.transient = false;
        frame.depth = scratch.depth.clone();
        replace_msaa(&mut frame.color, &scratch.msaa);
        if let Some(previous) = previous {
            let pass = &mut self.passes[previous];
            pass.depth = Some(scratch.depth);
            replace_msaa(&mut pass.color, &scratch.msaa);
        }
    }

    // ── draw planning (z decided by the caller) ─────────────────────────────

    fn plan_rect(&mut self, rect: &Rect, paint: &Paint, current: &Matrix, z: f32) {
        let folded = self.prepared_paint(paint);
        let paint = folded.as_ref().unwrap_or(paint);
        self.stats.draws += 1;
        if needs_effect_layer(paint) {
            // Only shader paints land here blurred (solid ones recorded the
            // analytic op) — general path: sharp draw in a layer, blur it.
            let local = paint.effect_bounds(*rect);
            let (rect2, paint2, current2) = (*rect, plain(paint), *current);
            self.plan_via_effect_layer(&local, paint, current, z, move |p| {
                p.plan_paint_quad(
                    PipelineKind::Draw(paint_frag(&paint2)),
                    &rect2,
                    &paint2,
                    &current2,
                    0.5,
                );
            });
            return;
        }
        if let Some(mode) = advanced_mode(paint) {
            if paint.shader.is_none() {
                self.plan_blend_solid_quad(
                    PipelineKind::Draw(Frag::BlendSolid),
                    rect,
                    paint,
                    current,
                    z,
                    mode,
                );
            } else {
                let device_bounds = current.map_rect(rect);
                let (rect2, paint2, current2) = (*rect, paint.clone(), *current);
                self.plan_via_implicit_layer(device_bounds, z, mode, move |p| {
                    p.plan_paint_quad(
                        PipelineKind::Draw(paint_frag(&paint2)),
                        &rect2,
                        &paint2,
                        &current2,
                        0.5,
                    );
                });
            }
            return;
        }
        self.plan_paint_quad(
            PipelineKind::Draw(paint_frag(paint)),
            rect,
            paint,
            current,
            z,
        );
    }

    /// Stencil-then-cover: wind the flattened path into the
    /// stencil, then one cover quad draws where wound. Any fragment family.
    fn plan_path_fill(
        &mut self,
        path: &Arc<Path>,
        rule: FillRule,
        paint: &Paint,
        current: &Matrix,
        z: f32,
    ) {
        let folded = self.prepared_paint(paint);
        let paint = folded.as_ref().unwrap_or(paint);
        self.stats.draws += 1;
        let bounds = path.bounds();
        if needs_effect_layer(paint) {
            let local = paint
                .effect_bounds(bounds.expand(paint.stroke_padding_at_scale(current.max_scale())));
            let path2 = path.clone();
            let (paint2, current2) = (plain(paint), *current);
            self.plan_via_effect_layer(&local, paint, current, z, move |p| {
                p.plan_path_geometry(&path2, rule, &paint2, &current2, 0.5);
            });
            return;
        }
        if let Some(mode) = advanced_mode(paint) {
            let solid_fill = paint.shader.is_none() && matches!(paint.style, PaintStyle::Fill);
            if solid_fill {
                let Some(mesh) = self.stencil_fan_mesh(path, current) else {
                    return;
                };
                self.push_fan(rule, current, mesh, z);
                self.plan_blend_solid_quad(
                    PipelineKind::Cover(Frag::BlendSolid),
                    &bounds,
                    paint,
                    current,
                    z,
                    mode,
                );
            } else {
                let padded = bounds.expand(paint.stroke_padding_at_scale(current.max_scale()));
                let device_bounds = current.map_rect(&padded);
                let path2 = path.clone();
                let (paint2, current2) = (plain(paint), *current);
                self.plan_via_implicit_layer(device_bounds, z, mode, move |p| {
                    p.plan_path_geometry(&path2, rule, &paint2, &current2, 0.5);
                });
            }
            return;
        }
        self.plan_path_geometry(path, rule, paint, current, z);
    }

    /// Fill via stencil-then-cover, or stroke via a CPU triangle strip —
    /// blur/blend already handled by the caller.
    fn plan_path_geometry(
        &mut self,
        path: &Arc<Path>,
        rule: FillRule,
        paint: &Paint,
        current: &Matrix,
        z: f32,
    ) {
        let stroke = match &paint.style {
            PaintStyle::Fill => {
                let Some(mesh) = self.stencil_fan_mesh(path, current) else {
                    return;
                };
                self.push_fan(rule, current, mesh, z);
                self.plan_paint_quad(
                    PipelineKind::Cover(paint_frag(paint)),
                    &path.bounds(),
                    paint,
                    current,
                    z,
                );
                return;
            }
            PaintStyle::Stroke(stroke) => stroke.clone(),
        };
        self.plan_path_stroke(path, &stroke, paint, current, z);
    }

    /// One `Strip` step along the flattened path (Impeller's
    /// StrokePathGeometry): dash pre-pass, hairline floor, joins + caps
    /// from the stroker. Gradients compose free (local = position).
    fn plan_path_stroke(
        &mut self,
        path: &Arc<Path>,
        stroke: &Stroke,
        paint: &Paint,
        current: &Matrix,
        z: f32,
    ) {
        let tolerance = local_tolerance(current);
        let contours = self.contours.contours(path, tolerance);
        // Impeller renders at least one device pixel of geometry, then fades
        // positive subpixel strokes to preserve their intended coverage. A
        // zero-width stroke is a true hairline and remains fully opaque.
        let mut stroke = stroke.clone();
        let coverage = stroke_alpha_coverage(current, stroke.width);
        stroke.width = stroke.width.max(1.0 / current.max_scale().max(1e-3));
        let vertices = match &stroke.dash {
            Some(dash) => {
                let dashed = dash_contours(&contours, dash);
                stroke_strip(&dashed, &stroke, tolerance)
            }
            None => stroke_strip(&contours, &stroke, tolerance),
        };
        if vertices.is_empty() {
            return;
        }
        let slot = self.host.alloc_vertices(bytemuck::cast_slice(&vertices));
        let mesh = (slot, (vertices.len() / 2) as u32);
        let tint = tinted(paint, self.elision_alpha() * coverage);
        let mut record = UniformRecord::new(self.ortho(current, z), tint);
        let bind = self.shader_payload(&mut record, paint);
        self.push_step(
            PipelineKind::Strip(paint_frag(paint)),
            paint.blend_mode,
            record,
            bind,
            Some(mesh),
            z,
        );
    }

    #[allow(clippy::too_many_arguments)] // mirrors the DrawImage op's fields 1:1
    fn plan_image(
        &mut self,
        image: &Image,
        src: &Rect,
        dst: &Rect,
        sampling: Sampling,
        paint: &Paint,
        current: &Matrix,
        z: f32,
    ) {
        self.stats.draws += 1;
        // Direct images filter the SAMPLED pixel in one draw, like Impeller's
        // ColorFilterAtlasContents. A blur still needs an effect layer so its
        // order remains sample → color filter → blur.
        if paint.mask_blur.is_some() || paint.effective_image_filter().is_some() {
            let local = paint.effect_bounds(*dst);
            let (image2, src2, dst2, paint2, current2) =
                (image.clone(), *src, *dst, plain(paint), *current);
            self.plan_via_effect_layer(&local, paint, current, z, move |p| {
                p.plan_image_step(&image2, &src2, &dst2, sampling, &paint2, &current2, 0.5);
            });
            return;
        }
        if let Some(mode) = advanced_mode(paint) {
            let device_bounds = current.map_rect(dst);
            let (image2, src2, dst2, paint2, current2) =
                (image.clone(), *src, *dst, paint.clone(), *current);
            self.plan_via_implicit_layer(device_bounds, z, mode, move |p| {
                p.plan_image_step(&image2, &src2, &dst2, sampling, &paint2, &current2, 0.5);
            });
            return;
        }
        self.plan_image_step(image, src, dst, sampling, paint, current, z);
    }

    fn filtered_image(&mut self, source: &Image, filter: ColorFilter) -> Image {
        let (filtered, created) = self.images.filtered_image(source, filter);
        if created {
            // Preserve ordering with any frame segment already accumulated;
            // the new independent pass then produces the cached texture.
            self.emit_segment();
            let whole = Rect::new(0.0, 0.0, source.width(), source.height());
            self.push_color_filter_to(
                source.view(),
                source.size(),
                &whole,
                filter,
                ColorFilterTarget {
                    view: filtered.view().clone(),
                    size: source.size(),
                    format: IMAGE_FORMAT,
                },
            );
        }
        filtered
    }

    fn prepared_paint(&mut self, paint: &Paint) -> Option<Paint> {
        if let Some(folded) = folded_paint(paint) {
            return Some(folded);
        }
        if paint.mask_blur.is_some() {
            return None;
        }
        let filter = paint.color_filter?;
        let Some(Shader::Image { image, .. }) = paint.shader.as_ref() else {
            return None;
        };
        // Image SHADERS match Impeller's TiledTextureContents: filter one
        // immutable source snapshot, then apply the pattern transform and
        // tile sampler. Direct drawImage takes the post-sampling atlas path.
        let filtered_image = self.filtered_image(image, filter);
        let mut prepared = paint.clone();
        let Some(Shader::Image { image, .. }) = prepared.shader.as_mut() else {
            unreachable!("source kind changed while cloning paint");
        };
        *image = filtered_image;
        prepared.color_filter = None;
        Some(prepared)
    }

    #[allow(clippy::too_many_arguments)]
    fn plan_image_step(
        &mut self,
        image: &Image,
        src: &Rect,
        dst: &Rect,
        sampling: Sampling,
        paint: &Paint,
        current: &Matrix,
        z: f32,
    ) {
        // ALPHA-only tint (samples × paint color would blacken under the
        // default black paint — learned once already).
        let model = current.then(&rect_to_unit(dst));
        let tint = alpha_tint(paint.color.a * self.elision_alpha());
        let mut record = UniformRecord::new(self.ortho(&model, z), tint);
        record.set_local_rect(dst);
        record.set_payload(PAYLOAD_GEOM, uv_mapping(image, src, dst));
        record.set_payload(PAYLOAD_DECAL, decal_flags(sampling));
        let fragment = match paint.color_filter {
            None => Frag::Image,
            Some(filter) => match encode_color_filter(&mut record, filter) {
                EncodedColorFilter::Matrix => Frag::ImageMatrix,
                EncodedColorFilter::Blend => Frag::ImageBlend,
            },
        };
        let bind = self
            .images
            .bind_group(self.pipelines.texture_bind_layout(), image, sampling);
        self.push_step(
            PipelineKind::Draw(fragment),
            paint.blend_mode,
            record,
            Some(bind),
            None,
            z,
        );
    }

    /// Depth clip: stencil the shape, write the depth CEILING at
    /// the clip's expiry z. Nothing to undo at restore.
    fn plan_clip(
        &mut self,
        path: &Arc<Path>,
        rule: FillRule,
        op: ClipOp,
        current: &Matrix,
        z: f32,
    ) {
        // A Difference ceiling covers the shape's INTERIOR — off-viewport
        // interiors exclude nothing visible, so skip the stencil+ceiling
        // work entirely. (Intersect ceilings cover the exterior and can
        // never be culled)
        if op == ClipOp::Difference {
            let visible = current
                .map_rect(&path.bounds())
                .intersects(&self.frame().cull_rect);
            if !visible {
                self.stats.culled += 1;
                return;
            }
        }
        let Some(mesh) = self.stencil_fan_mesh(path, current) else {
            // A zero-AREA path (e.g. a rect collapsed to a line) fans no
            // triangles. Intersecting with nothing clips EVERYTHING: with
            // no interior marked, the full-frame ceiling covers the whole
            // scope. An empty Difference excludes nothing — skip.
            if op == ClipOp::Intersect {
                self.stats.clips += 1;
                self.push_intersect_ceiling(z);
            }
            return;
        };
        self.stats.clips += 1;
        self.push_fan(rule, current, mesh, z);
        match op {
            ClipOp::Intersect => self.push_intersect_ceiling(z),
            ClipOp::Difference => {
                let model = current.then(&rect_to_unit(&path.bounds()));
                let record = UniformRecord::new(self.ortho(&model, z), [0.0; 4]);
                self.push_step(
                    PipelineKind::ClipCover { difference: true },
                    BlendMode::SrcOver,
                    record,
                    None,
                    None,
                    z,
                );
            }
        }
    }

    /// The Intersect ceiling: a full-frame cover in FRAME pixels (bypasses
    /// the origin shift) that writes expiry depth wherever the stencil left
    /// the shape's EXTERIOR — everything outside the clip fails depth until
    /// the scope closes.
    fn push_intersect_ceiling(&mut self, z: f32) {
        let viewport = self.frame_viewport();
        let record = UniformRecord::new(
            ortho_mvp(&rect_to_unit(&viewport), self.frame().size, z),
            [0.0; 4],
        );
        self.push_step(
            PipelineKind::ClipCover { difference: false },
            BlendMode::SrcOver,
            record,
            None,
            None,
            z,
        );
    }

    /// Desugar "draw X with an advanced blend" into a one-draw layer +
    /// BlendTexture composite (shader srcs become layers).
    /// `device_bounds` is in FRAME coords (post origin shift).
    fn plan_via_implicit_layer(
        &mut self,
        device_bounds: Rect,
        z: f32,
        mode: BlendMode,
        inner: impl FnOnce(&mut Self),
    ) {
        let Some(rect) = device_bounds.intersect(&self.frame().cull_rect) else {
            return;
        };
        self.stats.layers_rendered += 1;
        let paint = Paint {
            color: Color::WHITE,
            blend_mode: mode,
            ..Default::default()
        };
        self.push_layer_frame(rect, 2.0, paint, z, LayerEffects::default());
        inner(self);
        self.close_layer();
    }

    /// The general effect path: render the draw PLAIN into an implicit layer
    /// over its padded bounds, run the paint's colour filter and blur over
    /// that texture, and composite the result with the paint's blend
    /// (advanced modes keep their snapshot dance — `composite_layer` handles
    /// both).
    fn plan_via_effect_layer(
        &mut self,
        local_bounds: &Rect,
        paint: &Paint,
        current: &Matrix,
        z: f32,
        inner: impl FnOnce(&mut Self),
    ) {
        self.plan_via_effect_layer_at(current.map_rect(local_bounds), paint, current, z, inner);
    }

    /// Same, from device-space bounds (glyph runs carry theirs on the op —
    /// glyph extents aren't derivable at plan time).
    fn plan_via_effect_layer_at(
        &mut self,
        device_bounds: Rect,
        paint: &Paint,
        current: &Matrix,
        z: f32,
        inner: impl FnOnce(&mut Self),
    ) {
        let Some(rect) = device_bounds.intersect(&self.frame().cull_rect) else {
            return;
        };
        self.stats.layers_rendered += 1;
        let composite = Paint {
            color: Color::WHITE,
            blend_mode: paint.blend_mode,
            ..Default::default()
        };
        let effects = LayerEffects::of(paint, current.max_scale(), current, false);
        self.push_layer_frame(rect, 2.0, composite, z, effects);
        inner(self);
        self.close_layer();
    }

    /// The closed-form blurred (r)rect: ONE quad spanning the 3σ spread,
    /// coverage evaluated analytically (fs_rrect_blur) — no layer, no filter
    /// passes. Advanced blends wrap it in the usual implicit layer.
    fn plan_rrect_blur(
        &mut self,
        rect: &Rect,
        radii: [f32; 4],
        paint: &Paint,
        current: &Matrix,
        z: f32,
    ) {
        self.stats.draws += 1;
        if let Some(mode) = advanced_mode(paint) {
            let device_bounds = current.map_rect(&rect.expand(paint.mask_padding()));
            let (rect2, paint2, current2) = (*rect, paint.clone(), *current);
            self.plan_via_implicit_layer(device_bounds, z, mode, move |p| {
                p.push_rrect_blur_step(&rect2, radii, &paint2, &current2, 0.5);
            });
            return;
        }
        self.push_rrect_blur_step(rect, radii, paint, current, z);
    }

    fn push_rrect_blur_step(
        &mut self,
        rect: &Rect,
        radii: [f32; 4],
        paint: &Paint,
        current: &Matrix,
        z: f32,
    ) {
        let mask = paint.mask_blur.expect("recorded with mask_blur");
        let quad = rect.expand(paint.mask_padding());
        let model = current.then(&rect_to_unit(&quad));
        let tint = scaled_premul(paint.color, self.elision_alpha());
        let mut record = UniformRecord::new(self.ortho(&model, z), tint);
        record.set_local_rect(&quad);
        record.set_payload(
            PAYLOAD_GEOM,
            [rect.x, rect.y, rect.x + rect.width, rect.y + rect.height],
        );
        record.set_payload(
            PAYLOAD_MISC,
            [
                mask.sigma.max(0.05),
                blur_style_id(mask.style) as f32,
                0.0,
                0.0,
            ],
        );
        record.set_payload(PAYLOAD_RADII, radii);
        self.push_step(
            PipelineKind::Draw(Frag::RRectBlur),
            paint.blend_mode,
            record,
            None,
            None,
            z,
        );
    }

    // ── text: the three-tier picker ─────────────────────────────────────────

    /// One placed run of glyphs. Tier by DEVICE size: huge text fills real
    /// outlines (StC — always sharp); pixel-aligned 1:1 samples plain
    /// bitmaps (crispest); everything transformed goes through SDFs (one
    /// raster serves a range of scales/rotations).
    /// Skia's text tiers (SubRunControl.cpp): direct masks at
    /// quantized DEVICE scale below `sdf_min`, SDF buckets to `path_min`,
    /// real outlines beyond. Every tier respects the transform.
    /// Text draws like anything else: mask blur takes the blur-layer route
    /// (shadows), advanced blends the snapshot dance, then the tiers.
    #[allow(clippy::too_many_arguments)] // mirrors the GlyphRun op 1:1
    fn plan_glyph_run(
        &mut self,
        font: &Arc<Font>,
        size: f32,
        paint: &Paint,
        glyphs: &Arc<Vec<GlyphPos>>,
        device_bounds: Rect,
        current: &Matrix,
        z: f32,
    ) {
        self.stats.draws += 1;
        if needs_effect_layer(paint) {
            let (paint2, glyphs2, current2) = (plain(paint), glyphs.clone(), *current);
            let font = font.clone();
            self.plan_via_effect_layer_at(device_bounds, paint, current, z, move |p| {
                p.plan_glyph_tiers(&font, size, &paint2, &glyphs2, &current2, 0.5);
            });
            return;
        }
        if let Some(mode) = advanced_mode(paint) {
            let (paint2, glyphs2, current2) = (plain(paint), glyphs.clone(), *current);
            let font = font.clone();
            self.plan_via_implicit_layer(device_bounds, z, mode, move |p| {
                p.plan_glyph_tiers(&font, size, &paint2, &glyphs2, &current2, 0.5);
            });
            return;
        }
        if paint.shader.is_some() {
            self.plan_gradient_glyphs(font, size, paint, glyphs, device_bounds, current, z);
            return;
        }
        self.plan_glyph_tiers(font, size, paint, glyphs, current, z);
    }

    /// Shader-painted text desugars into the saveLayer recipe hosts would
    /// write by hand: glyphs as a white mask into an implicit layer, the
    /// shader `SrcIn` over the run (pipeline-blendable — no pass break),
    /// composite with the paint's blend. Every tier works unchanged.
    #[allow(clippy::too_many_arguments)]
    fn plan_gradient_glyphs(
        &mut self,
        font: &Arc<Font>,
        size: f32,
        paint: &Paint,
        glyphs: &Arc<Vec<GlyphPos>>,
        device_bounds: Rect,
        current: &Matrix,
        z: f32,
    ) {
        // The fill quad in LOCAL space (SrcIn masks it to the glyphs, so a
        // rotated superset rect is harmless).
        let local_quad = current
            .invert()
            .map_or(device_bounds, |inv| inv.map_rect(&device_bounds));
        let fill = Paint {
            shader: paint.shader.clone(),
            color: paint.color,
            blend_mode: BlendMode::SrcIn,
            ..Default::default()
        };
        let (glyphs2, current2) = (glyphs.clone(), *current);
        let style = paint.style.clone();
        self.plan_via_implicit_layer(device_bounds, z, paint.blend_mode, move |p| {
            // The mask must be drawn the way the paint asks — a stroked
            // gradient headline is stroked coverage, not a filled one.
            let mask = Paint {
                style: style.clone(),
                ..Paint::from_color(Color::WHITE)
            };
            p.plan_glyph_tiers(font, size, &mask, &glyphs2, &current2, 0.5);
            p.plan_paint_quad(
                PipelineKind::Draw(paint_frag(&fill)),
                &local_quad,
                &fill,
                &current2,
                0.5,
            );
        });
    }

    /// Skia's tier dispatch (SubRunControl.cpp): direct masks at
    /// quantized DEVICE scale below `sdf_min`, SDF buckets to `path_min`,
    /// real outlines beyond. Every tier respects the transform.
    fn plan_glyph_tiers(
        &mut self,
        font: &Arc<Font>,
        size: f32,
        paint: &Paint,
        glyphs: &[GlyphPos],
        current: &Matrix,
        z: f32,
    ) {
        let device_px = size * current.max_scale();
        let scale = quantize_scale(current.max_scale());
        match glyph_tier(self.glyphs.tiers, paint, scale, device_px) {
            GlyphTier::Outline => {
                self.stats.text_tiers[2] += 1;
                self.plan_glyph_outlines(font, size, paint, glyphs, current, z);
            }
            GlyphTier::Sdf => {
                self.stats.text_tiers[1] += 1;
                self.plan_glyph_quads(
                    font,
                    sdf_bucket(device_px),
                    Coverage::Sdf,
                    size,
                    paint,
                    glyphs,
                    current,
                    z,
                );
            }
            GlyphTier::Mask { coverage, alpha } => {
                self.stats.text_tiers[0] += 1;
                let paint = Paint {
                    color: paint.color.with_alpha(paint.color.a * alpha),
                    ..paint.clone()
                };
                self.plan_glyph_masks(font, size, scale, coverage, &paint, glyphs, current, z);
            }
        }
    }

    /// The mask tier's two shapes: device-snapped quads when the transform
    /// allows it, transformed quads over upright rasters otherwise
    /// (Impeller's shape).
    #[allow(clippy::too_many_arguments)] // mirrors plan_glyph_tiers
    fn plan_glyph_masks(
        &mut self,
        font: &Arc<Font>,
        size: f32,
        scale: f32,
        coverage: Coverage,
        paint: &Paint,
        glyphs: &[GlyphPos],
        current: &Matrix,
        z: f32,
    ) {
        if is_uniform_axis_aligned(current) {
            self.plan_glyph_quads_snapped(font, scale, size, coverage, paint, glyphs, current, z);
        } else {
            self.plan_glyph_quads(
                font,
                size * scale,
                coverage,
                size,
                paint,
                glyphs,
                current,
                z,
            );
        }
    }

    /// Atlas quads in LOCAL space (SDF tier, and rotated masks): glyphs
    /// rastered at `px`, placed at `size/px` of their raster dimensions,
    /// the transform applied by the MVP.
    #[allow(clippy::too_many_arguments)] // mirrors the GlyphRun op + tier
    fn plan_glyph_quads(
        &mut self,
        font: &Arc<Font>,
        px: f32,
        coverage: Coverage,
        size: f32,
        paint: &Paint,
        glyphs: &[GlyphPos],
        current: &Matrix,
        z: f32,
    ) {
        // Pack first, batch second — packing can GC the pages a batch
        // already points at (see GlyphStore::ensure_run). Hosts may opt to
        // hide glyph 0 (.notdef) and watch FontDemand instead of painting
        // tofu; the default draws it, like Skia.
        let hide_notdef = self.glyphs.hides_missing_glyphs();
        let keys: Vec<(u32, u8)> = glyphs
            .iter()
            .filter(|g| g.id != 0 || !hide_notdef)
            .map(|g| (g.id, 0))
            .collect();
        self.glyphs.ensure_run(font, px, coverage, &keys);
        let mut batches: Vec<((TextMode, PageRef), Vec<f32>)> = Vec::new();
        for g in glyphs.iter().filter(|g| g.id != 0 || !hide_notdef) {
            // Under a text-raster hold, a missing size draws through the
            // glyph's nearest resident size, scaled — the
            // per-glyph raster→quad scale makes the mixed-size batch free.
            let (got_px, page, entry) = match self.glyphs.entry(font.uid().0, g.id, px, coverage, 0)
            {
                Some((page, entry)) => (px, page, entry),
                None => match self
                    .glyphs
                    .resident_stand_in(font.uid().0, g.id, coverage, px)
                {
                    Some(hit) => hit,
                    None => continue,
                },
            };
            let batch = batch_for(&mut batches, text_mode(page, coverage), page);
            push_glyph_quad(batch, g.x, g.y, &entry, size / got_px);
        }
        self.push_text_batches(batches, paint, current, z);
    }

    /// The crisp path (mask tier, axis-aligned): glyphs rastered at the
    /// quantized device scale with a quarter-px subpixel phase, quads in
    /// DEVICE space 1:1 with their texels, y snapped to the pixel grid —
    /// Skia's direct masks / Impeller's quantized rasters.
    #[allow(clippy::too_many_arguments)]
    fn plan_glyph_quads_snapped(
        &mut self,
        font: &Arc<Font>,
        scale: f32,
        size: f32,
        coverage: Coverage,
        paint: &Paint,
        glyphs: &[GlyphPos],
        current: &Matrix,
        z: f32,
    ) {
        let px = size * scale;
        // Same optional .notdef policy as plan_glyph_quads.
        let hide_notdef = self.glyphs.hides_missing_glyphs();
        let placed: Vec<(f32, f32, u8, u32)> = glyphs
            .iter()
            .filter(|g| g.id != 0 || !hide_notdef)
            .map(|g| {
                let device = current.map_point(Point::new(g.x, g.y));
                let (x, phase) = snap_quarter(device.x);
                (x, device.y.round(), phase, g.id)
            })
            .collect();
        // Pack first, batch second (see GlyphStore::ensure_run).
        let keys: Vec<(u32, u8)> = placed
            .iter()
            .map(|&(_, _, phase, id)| (id, phase))
            .collect();
        self.glyphs.ensure_run(font, px, coverage, &keys);
        let mut batches: Vec<((TextMode, PageRef), Vec<f32>)> = Vec::new();
        for (x, y, phase, id) in placed {
            // Texels 1:1 when the exact scale is resident; under a hold, a
            // stand-in from another scale stretches (bitmaps
            // re-raster per quantize step, the very churn the hold skips).
            let (scale, page, entry) =
                match self.glyphs.entry(font.uid().0, id, px, coverage, phase) {
                    Some((page, entry)) => (1.0, page, entry),
                    None => match self
                        .glyphs
                        .resident_stand_in(font.uid().0, id, coverage, px)
                    {
                        Some((got_px, page, entry)) => (px / got_px, page, entry),
                        None => continue,
                    },
                };
            let batch = batch_for(&mut batches, text_mode(page, coverage), page);
            push_glyph_quad(batch, x, y, &entry, scale);
        }
        self.push_text_batches(batches, paint, &Matrix::IDENTITY, z);
    }

    /// One step per (mode, atlas page) batch, tinted per mode.
    fn push_text_batches(
        &mut self,
        batches: Vec<((TextMode, PageRef), Vec<f32>)>,
        paint: &Paint,
        model: &Matrix,
        z: f32,
    ) {
        for ((mode, page), vertices) in batches {
            let slot = self.host.alloc_vertices(bytemuck::cast_slice(&vertices));
            let mesh = (slot, (vertices.len() / 4) as u32);
            let tint = match mode {
                // Emoji keep their palette; only alpha rides the tint.
                TextMode::Color => alpha_tint(paint.color.a * self.elision_alpha()),
                _ => scaled_premul(paint.color, self.elision_alpha()),
            };
            let record = UniformRecord::new(self.ortho(model, z), tint);
            let bind = self
                .glyphs
                .bind_group(self.pipelines.texture_bind_layout(), page);
            self.push_step(
                PipelineKind::Text { mode },
                paint.blend_mode,
                record,
                Some(bind),
                Some(mesh),
                z,
            );
        }
    }

    /// The outline tier: each glyph is a real path, filled stencil-then-cover
    /// like any shape — no atlas entry could stay sharp this large.
    fn plan_glyph_outlines(
        &mut self,
        font: &Arc<Font>,
        size: f32,
        paint: &Paint,
        glyphs: &[GlyphPos],
        current: &Matrix,
        z: f32,
    ) {
        // Shader-painted text desugars into a layer before it gets here, so
        // outlines paint solid — but the STYLE rides along, which is what
        // makes stroked text stroke.
        let paint = Paint {
            color: paint.color,
            blend_mode: paint.blend_mode,
            style: paint.style.clone(),
            ..Default::default()
        };
        let hide_notdef = self.glyphs.hides_missing_glyphs();
        let mut no_outline: Vec<GlyphPos> = Vec::new();
        for g in glyphs.iter().filter(|g| g.id != 0 || !hide_notdef) {
            let Some(path) = self.glyphs.path(font, g.id, size) else {
                no_outline.push(*g);
                continue;
            };
            let at = current.then(&Matrix::translation(g.x, g.y));
            self.plan_path_geometry(&path, FillRule::NonZero, &paint, &at, z);
        }
        // Color glyphs (emoji) have no outlines — clamp them to the biggest
        // mask raster instead of vanishing.
        if !no_outline.is_empty() {
            let px = (size * current.max_scale()).min(MAX_COLOR_GLYPH_PX);
            let bitmap_paint = Paint {
                style: PaintStyle::Fill,
                ..paint.clone()
            };
            self.plan_glyph_quads(
                font,
                px,
                Coverage::Fill,
                size,
                &bitmap_paint,
                &no_outline,
                current,
                z,
            );
        }
    }

    // ── backdrop blur ───────────────────────────────────────────────────────

    /// Blur what's under the tile and composite it back at the tile's z
    /// (live depth clips shape it — that's the glass-panel look). Shared
    /// keys blur their union region once; later tiles just composite.
    #[allow(clippy::too_many_arguments)] // mirrors the BackdropBlur op 1:1
    fn plan_backdrop(
        &mut self,
        dl: &DisplayList,
        sigma_local: f32,
        shared_key: Option<u64>,
        base: &Matrix,
        current: &Matrix,
        bounds: &Rect,
        z: f32,
    ) {
        self.stats.draws += 1;
        let Some(tile) = base.map_rect(bounds).intersect(&self.frame().cull_rect) else {
            return;
        };
        // A key whose tiles disagree on σ never shares (recorded pre-pass,
        // like Impeller's FirstPassDispatcher counting backdrop_ids).
        let shared_key =
            shared_key.filter(|&k| dl.backdrop_group(k).is_some_and(|g| g.sigma.is_some()));
        let sigma = (sigma_local * current.max_scale()).max(0.05);
        if let Some(shared) = shared_key
            .and_then(|k| self.shared_blurs.get(&k))
            .filter(|s| (s.sigma - sigma).abs() < 1e-3)
        {
            // A later same-key tile: composite the existing blur, no break.
            // It shows the scene as of the FIRST tile — the shared-key trade.
            let (view, region, uv_max) = (shared.view.clone(), shared.region, shared.uv_max);
            self.push_backdrop_composite(&tile, &view, &region, uv_max, z);
            self.stats.shared_backdrops += 1;
            return;
        }
        let region = self.backdrop_blur_region(dl, shared_key, base, tile, sigma);
        let blur = self.blur_of_target_region(&region, sigma);
        if let Some(key) = shared_key {
            self.shared_blurs.insert(
                key,
                SharedBlur {
                    view: blur.view.clone(),
                    region,
                    uv_max: blur.uv_max,
                    sigma,
                },
            );
        }
        self.push_backdrop_composite(&tile, &blur.view, &region, blur.uv_max, z);
        self.stats.backdrops += 1;
    }

    /// What to blur, absolute coords: the tile (or a shared key's union of
    /// tiles), padded by 3σ so edge taps read real scene, clamped to frame.
    fn backdrop_blur_region(
        &mut self,
        dl: &DisplayList,
        shared_key: Option<u64>,
        base: &Matrix,
        tile: Rect,
        sigma: f32,
    ) -> Rect {
        let bounds = match shared_key.and_then(|k| dl.backdrop_group(k)) {
            Some(group) => base
                .map_rect(&group.union_bounds)
                .intersect(&self.frame().cull_rect)
                .unwrap_or(tile),
            None => tile,
        };
        let padded = bounds.expand((sigma * 3.0).ceil());
        padded.intersect(&self.frame().cull_rect).unwrap_or(bounds)
    }

    /// End the segment, snapshot `region` from the target, and blur it —
    /// the copy rides the blur chain's FIRST pass (which runs between this
    /// frame's segments), not the frame's next segment.
    fn blur_of_target_region(&mut self, region: &Rect, sigma: f32) -> FilteredTexture {
        self.emit_segment();
        self.stats.snapshots += 1;
        let (size, origin, src) = {
            let frame = self.frame();
            (frame.size, frame.origin, frame.src_texture.clone())
        };
        let snapshot = self.pool.take_snapshot(size, self.format);
        let mut copies = Vec::new();
        if let Some((copy_origin, extent)) = snapshot_region(region, origin, size) {
            copies.push(TextureCopy {
                src,
                dst: snapshot.texture.clone(),
                origin: copy_origin,
                size: extent,
            });
        }
        let local = Rect::new(
            region.x - origin.x,
            region.y - origin.y,
            region.width,
            region.height,
        );
        self.plan_blur(&snapshot.view, size, &local, sigma, copies)
    }

    fn push_backdrop_composite(
        &mut self,
        tile: &Rect,
        view: &wgpu::TextureView,
        region: &Rect,
        uv_max: [f32; 2],
        z: f32,
    ) {
        let mut record = self.quad_record(tile, [1.0, 1.0, 1.0, 1.0], z);
        record.set_payload(PAYLOAD_GEOM, region_uv(region, uv_max));
        let bind = self.texture_bind(view);
        self.push_step(
            PipelineKind::Draw(Frag::Image),
            BlendMode::SrcOver,
            record,
            Some(bind),
            None,
            z,
        );
    }

    // ── gaussian blur chains (blur at scale) ────────────────────────────────

    /// Blur `region` (px inside `source`, sized `source_size`): downsample
    /// until the effective σ is ≤ ~4, then one horizontal + one vertical
    /// separable pass; the composite's bilinear sampling upscales for free.
    /// Passes append to the plan at the CURRENT position — callers emit
    /// their frame's segment first.
    fn plan_blur(
        &mut self,
        source: &wgpu::TextureView,
        source_size: [u32; 2],
        region: &Rect,
        sigma: f32,
        pre_copies: Vec<TextureCopy>,
    ) -> FilteredTexture {
        let scale = blur_scale(sigma);
        let work = [
            (region.width * scale).round().max(1.0),
            (region.height * scale).round().max(1.0),
        ];
        let mut copies = pre_copies;
        let mut src = source.clone();
        let mut src_px = [source_size[0] as f32, source_size[1] as f32];
        // `None` while the source is still the caller's texture, where the
        // content is a sub-rect; `Some(uv_max)` once it is a pooled target
        // whose content starts at the origin.
        let mut src_uv_max: Option<[f32; 2]> = None;
        // Downsample passes (σ=0 blur = plain bilinear resample), halving at
        // most 2× each so bilinear reads every texel — one big jump would
        // alias exactly what the blur is meant to average.
        let mut cur = [region.width, region.height];
        while scale < 1.0 && (cur[0] > work[0] || cur[1] > work[1]) {
            let next = [
                (cur[0] * 0.5).max(work[0]).round().max(1.0),
                (cur[1] * 0.5).max(work[1]).round().max(1.0),
            ];
            let uv = match src_uv_max {
                None => source_region_uv(region, source_size, next),
                Some(uv_max) => resample_uv(uv_max, next),
            };
            let (view, bucket) =
                self.push_filter_pass(&src, uv, next, 0.0, [0.0, 0.0], std::mem::take(&mut copies));
            src = view;
            src_px = bucket;
            src_uv_max = Some([next[0] / bucket[0], next[1] / bucket[1]]);
            cur = next;
        }
        let work_sigma = sigma * scale;
        let src_uv = match src_uv_max {
            None => source_region_uv(region, source_size, work),
            Some(uv_max) => resample_uv(uv_max, work),
        };
        let (h_view, h_bucket) = self.push_filter_pass(
            &src,
            src_uv,
            work,
            work_sigma,
            [1.0 / src_px[0], 0.0],
            std::mem::take(&mut copies),
        );
        let (v_view, v_bucket) = self.push_filter_pass(
            &h_view,
            corner_uv(h_bucket),
            work,
            work_sigma,
            [0.0, 1.0 / h_bucket[1]],
            Vec::new(),
        );
        FilteredTexture {
            view: v_view,
            uv_max: [work[0] / v_bucket[0], work[1] / v_bucket[1]],
            size: [v_bucket[0] as u32, v_bucket[1] as u32],
        }
    }

    /// Blur a semantic full-size filter input whose used pixels may occupy
    /// only a bucket corner. The UV mapping carries that resolution through
    /// an ordered image-filter chain without stretching intermediate output.
    fn plan_blur_input(
        &mut self,
        source: &FilteredTexture,
        whole: &Rect,
        sigma_x: f32,
        sigma_y: f32,
    ) -> FilteredTexture {
        let scale_x = blur_scale(sigma_x);
        let scale_y = blur_scale(sigma_y);
        let work = [
            (whole.width * scale_x).round().max(1.0),
            (whole.height * scale_y).round().max(1.0),
        ];
        let mut source_view = source.view.clone();
        let mut source_pixels = [source.size[0] as f32, source.size[1] as f32];
        let mut source_uv_max = source.uv_max;
        let mut current = [whole.width, whole.height];
        while current[0] > work[0] || current[1] > work[1] {
            let next = [
                (current[0] * 0.5).max(work[0]).round().max(1.0),
                (current[1] * 0.5).max(work[1]).round().max(1.0),
            ];
            let (view, bucket) = self.push_filter_pass(
                &source_view,
                resample_uv(source_uv_max, next),
                next,
                0.0,
                [0.0, 0.0],
                Vec::new(),
            );
            source_view = view;
            source_pixels = bucket;
            source_uv_max = [next[0] / bucket[0], next[1] / bucket[1]];
            current = next;
        }
        let (horizontal, horizontal_bucket) = self.push_filter_pass(
            &source_view,
            resample_uv(source_uv_max, work),
            work,
            sigma_x * scale_x,
            [1.0 / source_pixels[0], 0.0],
            Vec::new(),
        );
        let (vertical, vertical_bucket) = self.push_filter_pass(
            &horizontal,
            corner_uv(horizontal_bucket),
            work,
            sigma_y * scale_y,
            [0.0, 1.0 / horizontal_bucket[1]],
            Vec::new(),
        );
        FilteredTexture {
            view: vertical,
            uv_max: [work[0] / vertical_bucket[0], work[1] / vertical_bucket[1]],
            size: [vertical_bucket[0] as u32, vertical_bucket[1] as u32],
        }
    }

    fn push_image_filter(
        &mut self,
        source: &FilteredTexture,
        whole: &Rect,
        filter: &ImageFilter,
        basis: [f32; 4],
    ) -> FilteredTexture {
        let mut stages = Vec::new();
        image_filter_stages(filter, &mut stages);
        let mut output = source.clone();
        let mut index = 0;
        while index < stages.len() {
            match stages[index] {
                ImageFilter::Color(filter) => {
                    output = self.push_color_filter_input(&output, whole, *filter);
                    index += 1;
                }
                ImageFilter::Blur { .. } => {
                    // The basis has to reach every stage BEFORE they combine.
                    // Impeller transforms each blur's σ on its own —
                    // `CalculateBlurInfo` runs per filter — and a rotation
                    // MIXES the axes, so combining in local space first and
                    // rotating the total afterwards blurs along the wrong
                    // ones. Successive gaussians still compose in quadrature
                    // once they are on the same axes, so the merged device σ
                    // collapses back to a single pass.
                    let mut sigma_x_squared = 0.0;
                    let mut sigma_y_squared = 0.0;
                    while let Some(ImageFilter::Blur { sigma_x, sigma_y }) = stages.get(index) {
                        let [device_x, device_y] = device_sigma(basis, *sigma_x, *sigma_y);
                        sigma_x_squared += device_x * device_x;
                        sigma_y_squared += device_y * device_y;
                        index += 1;
                    }
                    let (sigma_x, sigma_y) = (sigma_x_squared.sqrt(), sigma_y_squared.sqrt());
                    if sigma_x <= 0.0 && sigma_y <= 0.0 {
                        continue;
                    }
                    output = self.plan_blur_input(&output, whole, sigma_x, sigma_y);
                }
                ImageFilter::DropShadow {
                    offset,
                    sigma_x,
                    sigma_y,
                    color,
                } => {
                    output = self.plan_drop_shadow(
                        &output,
                        whole,
                        *offset,
                        [*sigma_x, *sigma_y],
                        *color,
                        basis,
                    );
                    index += 1;
                }
                ImageFilter::Compose { .. } => unreachable!("composition was flattened"),
            }
        }
        output
    }

    fn materialize_filter_input(
        &mut self,
        source: &FilteredTexture,
        whole: &Rect,
    ) -> FilteredTexture {
        let expected = [
            whole.width / source.size[0] as f32,
            whole.height / source.size[1] as f32,
        ];
        if (source.uv_max[0] - expected[0]).abs() < 1e-6
            && (source.uv_max[1] - expected[1]).abs() < 1e-6
        {
            return source.clone();
        }
        let work = [whole.width, whole.height];
        let (view, bucket) = self.push_filter_pass(
            &source.view,
            region_uv(whole, source.uv_max),
            work,
            0.0,
            [0.0, 0.0],
            Vec::new(),
        );
        FilteredTexture {
            view,
            uv_max: [work[0] / bucket[0], work[1] / bucket[1]],
            size: [bucket[0] as u32, bucket[1] as u32],
        }
    }

    /// One quad filling an exactly-sized 1-sample target; fs_blur taps
    /// `source` along `step` (radius 0 = plain resample).
    fn push_filter_pass(
        &mut self,
        source: &wgpu::TextureView,
        source_uv: [f32; 4],
        work: [f32; 2],
        sigma: f32,
        step: [f32; 2],
        pre_copies: Vec<TextureCopy>,
    ) -> (wgpu::TextureView, [f32; 2]) {
        let extent = [exact_extent(work[0]), exact_extent(work[1])];
        let target = self.pool.take_filter(extent, self.format);
        let quad = Rect::new(0.0, 0.0, work[0], work[1]);
        let radius = if sigma > 0.0 {
            (sigma * 2.5).ceil().min(48.0)
        } else {
            0.0
        };
        let mut record = UniformRecord::new(ortho_mvp(&rect_to_unit(&quad), extent, 0.0), [0.0; 4]);
        record.set_local_rect(&quad);
        record.set_payload(PAYLOAD_GEOM, source_uv);
        record.set_payload(PAYLOAD_MISC, [sigma, radius, step[0], step[1]]);
        let bind = self.texture_bind(source);
        self.push_filter(target.view.clone(), Frag::Blur, record, bind, pre_copies);
        (target.view, [extent[0] as f32, extent[1] as f32])
    }

    /// Recolour a raw layer texture in one filter pass.
    fn push_color_filter(
        &mut self,
        source: &wgpu::TextureView,
        source_size: [u32; 2],
        whole: &Rect,
        filter: ColorFilter,
    ) -> FilteredTexture {
        let work = [whole.width, whole.height];
        let bucket = [filter_bucket(work[0]), filter_bucket(work[1])];
        let target = self.pool.take_filter(bucket, self.format);
        self.push_color_filter_to(
            source,
            source_size,
            whole,
            filter,
            ColorFilterTarget {
                view: target.view.clone(),
                size: bucket,
                format: self.format,
            },
        );
        FilteredTexture {
            view: target.view,
            uv_max: [work[0] / bucket[0] as f32, work[1] / bucket[1] as f32],
            size: bucket,
        }
    }

    /// Recolour a layer texture while preserving the used corner of a
    /// bucketed or downsampled prior stage.
    fn push_color_filter_input(
        &mut self,
        source: &FilteredTexture,
        whole: &Rect,
        filter: ColorFilter,
    ) -> FilteredTexture {
        let work = [whole.width, whole.height];
        let bucket = [filter_bucket(work[0]), filter_bucket(work[1])];
        let target = self.pool.take_filter(bucket, self.format);
        let mut record = UniformRecord::new(ortho_mvp(&rect_to_unit(whole), bucket, 0.0), [0.0; 4]);
        record.set_local_rect(whole);
        record.set_payload(PAYLOAD_GEOM, region_uv(whole, source.uv_max));
        let fragment = match encode_color_filter(&mut record, filter) {
            EncodedColorFilter::Matrix => Frag::ColorMatrix,
            EncodedColorFilter::Blend => Frag::ColorBlend,
        };
        let bind = self.texture_bind(&source.view);
        self.push_filter(target.view.clone(), fragment, record, bind, Vec::new());
        FilteredTexture {
            view: target.view,
            uv_max: [work[0] / bucket[0] as f32, work[1] / bucket[1] as f32],
            size: bucket,
        }
    }

    fn push_color_filter_to(
        &mut self,
        source: &wgpu::TextureView,
        source_size: [u32; 2],
        whole: &Rect,
        filter: ColorFilter,
        target: ColorFilterTarget,
    ) {
        let mut record =
            UniformRecord::new(ortho_mvp(&rect_to_unit(whole), target.size, 0.0), [0.0; 4]);
        record.set_local_rect(whole);
        record.set_payload(
            PAYLOAD_GEOM,
            source_region_uv(whole, source_size, [whole.width, whole.height]),
        );
        let frag = match encode_color_filter(&mut record, filter) {
            EncodedColorFilter::Matrix => Frag::ColorMatrix,
            EncodedColorFilter::Blend => Frag::ColorBlend,
        };
        let bind = self.texture_bind(source);
        self.push_filter_with_format(target.view, target.format, frag, record, bind, Vec::new());
    }

    /// Skia's `SkImageFilters::DropShadow` lowering: tint the input's alpha
    /// with the shadow colour, blur it, displace it, and put the untouched
    /// input back on top. The blur runs on the tinted copy rather than the
    /// input so a translucent shadow colour spreads at its own alpha.
    fn plan_drop_shadow(
        &mut self,
        source: &FilteredTexture,
        whole: &Rect,
        offset: Point,
        sigma: [f32; 2],
        color: Color,
        basis: [f32; 4],
    ) -> FilteredTexture {
        let tint = ColorFilter::Blend(color, BlendMode::SrcIn);
        let tinted = self.push_color_filter_input(source, whole, tint);
        let [device_x, device_y] = skia_sigma(basis, sigma[0], sigma[1]);
        let shadow = if device_x > 0.0 || device_y > 0.0 {
            self.plan_blur_input(&tinted, whole, device_x, device_y)
        } else {
            tinted
        };
        self.push_drop_shadow_combine(&shadow, source, whole, device_offset(basis, offset))
    }

    /// Merge the offset shadow B with the sharp layer S into one texture
    /// (fs_drop_shadow), so the result composites like any other layer.
    fn push_drop_shadow_combine(
        &mut self,
        shadow: &FilteredTexture,
        sharp: &FilteredTexture,
        whole: &Rect,
        offset: [f32; 2],
    ) -> FilteredTexture {
        let work = [whole.width, whole.height];
        let bucket = [filter_bucket(work[0]), filter_bucket(work[1])];
        let target = self.pool.take_filter(bucket, self.format);
        let mut record = UniformRecord::new(ortho_mvp(&rect_to_unit(whole), bucket, 0.0), [0.0; 4]);
        record.set_local_rect(whole);
        record.set_payload(
            PAYLOAD_GEOM,
            [
                shadow.uv_max[0] / work[0],
                shadow.uv_max[1] / work[1],
                sharp.uv_max[0] / work[0],
                sharp.uv_max[1] / work[1],
            ],
        );
        record.set_payload(
            PAYLOAD_MISC,
            [offset[0], offset[1], 1.0 / work[0], 1.0 / work[1]],
        );
        let bind = self.blend_bind(&shadow.view, &sharp.view);
        self.push_filter(
            target.view.clone(),
            Frag::DropShadow,
            record,
            bind,
            Vec::new(),
        );
        FilteredTexture {
            view: target.view,
            uv_max: [work[0] / bucket[0] as f32, work[1] / bucket[1] as f32],
            size: bucket,
        }
    }

    /// Merge blur B with the SHARP layer M into one texture (fs_mask_combine)
    /// so styled masks composite like any other draw — advanced blends too.
    fn push_mask_combine(
        &mut self,
        blur: &FilteredTexture,
        sharp: &wgpu::TextureView,
        whole: &Rect,
        style: BlurStyle,
    ) -> FilteredTexture {
        let work = [whole.width, whole.height];
        let bucket = [filter_bucket(work[0]), filter_bucket(work[1])];
        let target = self.pool.take_filter(bucket, self.format);
        let mut record = UniformRecord::new(ortho_mvp(&rect_to_unit(whole), bucket, 0.0), [0.0; 4]);
        record.set_local_rect(whole);
        record.set_payload(
            PAYLOAD_GEOM,
            [blur.uv_max[0] / work[0], blur.uv_max[1] / work[1], 0.0, 0.0],
        );
        record.set_payload(
            PAYLOAD_MISC,
            [
                blur_style_id(style) as f32,
                0.0,
                1.0 / work[0],
                1.0 / work[1],
            ],
        );
        let bind = self.blend_bind(&blur.view, sharp);
        self.push_filter(
            target.view.clone(),
            Frag::MaskCombine,
            record,
            bind,
            Vec::new(),
        );
        FilteredTexture {
            view: target.view,
            uv_max: [work[0] / bucket[0] as f32, work[1] / bucket[1] as f32],
            size: bucket,
        }
    }

    /// One-quad filter pass appended to the plan at the current position.
    fn push_filter(
        &mut self,
        target: wgpu::TextureView,
        frag: Frag,
        record: UniformRecord,
        bind: wgpu::BindGroup,
        pre_copies: Vec<TextureCopy>,
    ) {
        self.push_filter_with_format(target, self.format, frag, record, bind, pre_copies);
    }

    fn push_filter_with_format(
        &mut self,
        target: wgpu::TextureView,
        target_format: wgpu::TextureFormat,
        frag: Frag,
        record: UniformRecord,
        bind: wgpu::BindGroup,
        pre_copies: Vec<TextureCopy>,
    ) {
        let uniforms = self.host.alloc_uniform(&record.bytes);
        let key = PipelineKey::new(
            target_format,
            BlendMode::SrcOver,
            PipelineKind::Filter(frag),
        );
        self.passes.push(PlannedPass {
            color: PassColor::Filter { view: target },
            depth: None,
            clear: Some(Color::TRANSPARENT),
            clear_depth: false,
            store: true, // the next pass samples this
            pre_copies,
            steps: vec![Step {
                key,
                uniforms,
                texture: Some(bind),
                mesh: None,
                sort_z: 0.0,
            }],
        });
        self.stats.filter_passes += 1;
    }

    // ── step building blocks ────────────────────────────────────────────────

    /// The occluder gate: a paint that provably covers every pixel
    /// it touches gets the depth-writing pipeline so the reorder pass can
    /// hoist it and early-z can cull what's underneath.
    fn promote_opaque(&self, kind: PipelineKind, paint: &Paint) -> PipelineKind {
        if self.elision_alpha() < 1.0 || !is_opaque_paint(paint) {
            return kind;
        }
        match kind {
            PipelineKind::Draw(f) => PipelineKind::OpaqueDraw(f),
            PipelineKind::Cover(f) => PipelineKind::OpaqueCover(f),
            other => other,
        }
    }

    fn plan_paint_quad(
        &mut self,
        kind: PipelineKind,
        quad: &Rect,
        paint: &Paint,
        current: &Matrix,
        z: f32,
    ) {
        let (record, bind) = self.paint_quad_record(quad, paint, current, z);
        let kind = self.promote_opaque(kind, paint);
        self.push_step(kind, paint.blend_mode, record, bind, None, z);
    }

    fn plan_blend_solid_quad(
        &mut self,
        kind: PipelineKind,
        quad: &Rect,
        paint: &Paint,
        current: &Matrix,
        z: f32,
        mode: BlendMode,
    ) {
        let snapshot = self.break_pass(&current.map_rect(quad));
        let model = current.then(&rect_to_unit(quad));
        let tint = scaled_premul(paint.color, self.elision_alpha());
        let mut record = UniformRecord::new(self.ortho(&model, z), tint);
        record.set_local_rect(quad);
        self.set_blend_misc(&mut record, mode);
        let bind = self.texture_bind(&snapshot);
        self.push_step(kind, BlendMode::SrcOver, record, Some(bind), None, z);
    }

    /// mvp + tint + local rect (+ gradient payload when the paint has
    /// one). >8-stop gradients also return the baked RAMP texture's bind
    /// (Impeller's texture-gradient path).
    fn paint_quad_record(
        &mut self,
        quad: &Rect,
        paint: &Paint,
        current: &Matrix,
        z: f32,
    ) -> (UniformRecord, Option<wgpu::BindGroup>) {
        let model = current.then(&rect_to_unit(quad));
        let tint = tinted(paint, self.elision_alpha());
        let mut record = UniformRecord::new(self.ortho(&model, z), tint);
        record.set_local_rect(quad);
        let bind = self.shader_payload(&mut record, paint);
        (record, bind)
    }

    /// Fill the paint's shader payload, returning the texture bind the draw
    /// must carry: a baked ramp for stop lists past the uniform budget, the
    /// image itself for a pattern, nothing for the rest.
    fn shader_payload(
        &mut self,
        record: &mut UniformRecord,
        paint: &Paint,
    ) -> Option<wgpu::BindGroup> {
        match paint.shader.as_ref()? {
            Shader::Image {
                image,
                sampling,
                local,
            } => {
                fill_pattern_payload(record, image, *sampling, local);
                Some(
                    self.images
                        .bind_group(self.pipelines.texture_bind_layout(), image, *sampling),
                )
            }
            shader => {
                let ramp = (shader.stops().len() > MAX_GRADIENT_STOPS).then(|| {
                    let (view, texels) = self.ramps.ensure(self.device, self.queue, shader.stops());
                    (self.texture_bind(&view), texels)
                });
                fill_gradient_payload(record, shader, ramp.as_ref().map(|(_, n)| *n));
                ramp.map(|(bind, _)| bind)
            }
        }
    }

    /// A quad at an ABSOLUTE rect (composites) — origin-shifted like any
    /// draw, no paint machinery.
    fn quad_record(&mut self, rect: &Rect, tint: [f32; 4], z: f32) -> UniformRecord {
        let mut record = UniformRecord::new(self.ortho(&rect_to_unit(rect), z), tint);
        record.set_local_rect(rect);
        record
    }

    fn push_fan(&mut self, rule: FillRule, current: &Matrix, mesh: (VertexSlot, u32), z: f32) {
        let record = UniformRecord::new(self.ortho(current, 0.0), [0.0; 4]);
        self.push_step(
            fan_kind(rule),
            BlendMode::SrcOver,
            record,
            None,
            Some(mesh),
            z,
        );
    }

    #[allow(clippy::too_many_arguments)] // the one funnel every draw goes through
    fn push_step(
        &mut self,
        kind: PipelineKind,
        blend: BlendMode,
        record: UniformRecord,
        texture: Option<wgpu::BindGroup>,
        mesh: Option<(VertexSlot, u32)>,
        z: f32,
    ) {
        let uniforms = self.host.alloc_uniform(&record.bytes);
        let key = PipelineKey::new(self.format, blend, kind);
        self.frame_mut().steps.push(Step {
            key,
            uniforms,
            texture,
            mesh,
            sort_z: z,
        });
    }

    /// Flatten at the draw's device scale and fan every contour from its
    /// first point (winding fixes coverage — triangles may overlap freely).
    fn stencil_fan_mesh(
        &mut self,
        path: &Arc<Path>,
        current: &Matrix,
    ) -> Option<(VertexSlot, u32)> {
        let contours = self.contours.contours(path, local_tolerance(current));
        let vertices = fan_vertices(&contours);
        if vertices.is_empty() {
            return None;
        }
        let slot = self.host.alloc_vertices(bytemuck::cast_slice(&vertices));
        Some((slot, (vertices.len() / 2) as u32))
    }

    fn texture_bind(&self, view: &wgpu::TextureView) -> wgpu::BindGroup {
        self.device.create_bind_group(&wgpu::BindGroupDescriptor {
            label: Some("valo.step.texture"),
            layout: self.pipelines.texture_bind_layout(),
            entries: &[
                wgpu::BindGroupEntry {
                    binding: 0,
                    resource: wgpu::BindingResource::TextureView(view),
                },
                wgpu::BindGroupEntry {
                    binding: 1,
                    resource: wgpu::BindingResource::Sampler(&self.sampler),
                },
            ],
        })
    }

    fn blend_bind(&self, dst: &wgpu::TextureView, src: &wgpu::TextureView) -> wgpu::BindGroup {
        self.device.create_bind_group(&wgpu::BindGroupDescriptor {
            label: Some("valo.step.blend"),
            layout: self.pipelines.blend_bind_layout(),
            entries: &[
                wgpu::BindGroupEntry {
                    binding: 0,
                    resource: wgpu::BindingResource::TextureView(dst),
                },
                wgpu::BindGroupEntry {
                    binding: 1,
                    resource: wgpu::BindingResource::Sampler(&self.sampler),
                },
                wgpu::BindGroupEntry {
                    binding: 2,
                    resource: wgpu::BindingResource::TextureView(src),
                },
            ],
        })
    }

    fn set_blend_misc(&mut self, record: &mut UniformRecord, mode: BlendMode) {
        let size = self.frame().size;
        record.set_payload(
            PAYLOAD_MISC,
            [
                advanced_mode_id(mode) as f32,
                0.0,
                size[0] as f32,
                size[1] as f32,
            ],
        );
    }

    // ── small helpers ───────────────────────────────────────────────────────

    fn frame(&self) -> &PassFrame {
        self.frames.last().expect("frame stack never empty")
    }

    fn frame_mut(&mut self) -> &mut PassFrame {
        self.frames.last_mut().expect("frame stack never empty")
    }

    fn frame_viewport(&self) -> Rect {
        let s = self.frame().size;
        Rect::new(0.0, 0.0, s[0] as f32, s[1] as f32)
    }

    fn slot_z(&self, slot: u32) -> f32 {
        (self.slot_offset + slot as i64) as f32 / self.frame().z_denom
    }

    fn elision_alpha(&self) -> f32 {
        self.elisions.last().copied().unwrap_or(1.0)
    }

    /// The MVP for the current frame: transforms live in the TOP pass's
    /// coords; layer frames subtract their accumulated origin so children
    /// land in layer pixels.
    fn ortho(&self, m: &Matrix, z: f32) -> [f32; 16] {
        let o = self.frame().origin;
        let shifted = Matrix::translation(-o.x, -o.y).then(m);
        ortho_mvp(&shifted, self.frame().size, z)
    }
}

enum Opened {
    Skip,
    Elided,
    Layer,
}

/// Skip a save/saveLayer scope's ops (used when a layer is invisible):
/// returns the index of the matching Restore.
fn skip_scope(ops: &[Op], open_index: usize) -> usize {
    let mut depth = 0usize;
    let mut i = open_index;
    loop {
        match &ops[i] {
            Op::Save | Op::SaveLayer { .. } => depth += 1,
            Op::Restore => {
                depth -= 1;
                if depth == 0 {
                    return i;
                }
            }
            _ => {}
        }
        i += 1;
    }
}

/// What a layer's composite will draw with — carried from the SaveLayer op
/// through the frame push into [`LayerInfo`].
struct Composite {
    paint: Paint,
    mask: Option<MaskKind>,
}

/// The record-time facts a SaveLayer carries. The oracle computed every one of
/// these, so opening a layer reads them instead of deriving anything.
struct LayerScope<'a> {
    bounds: &'a Rect,
    base_slot: u32,
    composite_slot: u32,
    can_elide: bool,
}

fn advanced_mode(paint: &Paint) -> Option<BlendMode> {
    (!paint.blend_mode.is_pipeline_blendable()).then_some(paint.blend_mode)
}

fn paint_frag(paint: &Paint) -> Frag {
    let ramp = paint
        .shader
        .as_ref()
        .is_some_and(|s| s.stops().len() > MAX_GRADIENT_STOPS);
    match &paint.shader {
        None => Frag::Solid,
        Some(Shader::Linear { .. }) if ramp => Frag::LinearRamp,
        Some(Shader::Radial { .. }) if ramp => Frag::RadialRamp,
        Some(Shader::Sweep { .. }) if ramp => Frag::SweepRamp,
        Some(Shader::Linear { .. }) => Frag::Linear,
        Some(Shader::Radial { .. }) => Frag::Radial,
        Some(Shader::Sweep { .. }) => Frag::Sweep,
        Some(Shader::Image { .. }) => Frag::Pattern,
    }
}

/// What multiplies the fragment family's output. Solid = the color itself;
/// image/gradient sources use paint ALPHA only (Skia's drawImage semantics).
/// `extra` folds in an elided group's alpha.
fn tinted(paint: &Paint, extra: f32) -> [f32; 4] {
    if paint.shader.is_none() {
        scaled_premul(paint.color, extra)
    } else {
        alpha_tint(paint.color.a * extra)
    }
}

/// Encode Impeller's mat4-plus-translation-vector layout, or its constant
/// premultiplied blend source. Draw and filter-pass shaders share this ABI.
fn encode_color_filter(record: &mut UniformRecord, filter: ColorFilter) -> EncodedColorFilter {
    match filter {
        ColorFilter::Matrix(matrix) => {
            for row in 0..4 {
                let start = row * 5;
                record.set_payload(
                    PAYLOAD_COLOR_MATRIX + row,
                    [
                        matrix[start],
                        matrix[start + 1],
                        matrix[start + 2],
                        matrix[start + 3],
                    ],
                );
            }
            record.set_payload(
                PAYLOAD_COLOR_MATRIX + 4,
                [matrix[4], matrix[9], matrix[14], matrix[19]],
            );
            EncodedColorFilter::Matrix
        }
        ColorFilter::Blend(color, mode) => {
            record.set_payload(PAYLOAD_COLOR_MATRIX, color.premultiplied());
            record.set_payload(PAYLOAD_MISC, [blend_filter_id(mode) as f32, 0.0, 0.0, 0.0]);
            EncodedColorFilter::Blend
        }
    }
}

/// Impeller's `Geometry::ComputeStrokeAlphaCoverage`: geometry below one
/// device pixel is widened, so positive-width strokes compensate in alpha.
/// Width zero deliberately means a fully covered one-pixel hairline.
fn stroke_alpha_coverage(transform: &Matrix, width: f32) -> f32 {
    subpixel_stroke_alpha(transform.max_scale() * width)
}

/// The same compensation for a width already in device pixels — the mask
/// tier floors its raster width instead of its geometry.
fn subpixel_stroke_alpha(device_width: f32) -> f32 {
    if device_width == 0.0 || device_width >= 1.0 {
        1.0
    } else {
        (device_width * 2.0).clamp(0.0, 1.0)
    }
}

fn scaled_premul(color: Color, alpha: f32) -> [f32; 4] {
    let [r, g, b, a] = color.premultiplied();
    [r * alpha, g * alpha, b * alpha, a * alpha]
}

fn alpha_tint(a: f32) -> [f32; 4] {
    [a, a, a, a]
}

/// Which formula the fragment runs for a radial gradient, plus the constants
/// it needs. Skia's two-point conical algorithm (skia.org/docs/dev/design/
/// conical) splits into cases by where the focal point lands; the choice and
/// all of its precomputation are per-draw, so they happen here rather than
/// per fragment the way Impeller does it.
struct ConicalSetup {
    /// `(kind, local_r1, f, d_radius_sign)` — see `radial_t` in the shader.
    constants: [f32; 4],
    /// `(is_swapped, is_focal_on_circle, is_well_behaved, unused)`.
    flags: [f32; 4],
    /// Gradient space → focal space, when the general case needs it.
    focal_map: Option<Matrix>,
}

/// Kinds, mirrored in `radial_t`.
const CONICAL_CONCENTRIC: f32 = 0.0;
const CONICAL_GENERAL: f32 = 1.0;
const CONICAL_EMPTY: f32 = 2.0;
const CONICAL_STRIP: f32 = 3.0;

impl ConicalSetup {
    const UNUSED: Self = Self {
        constants: [CONICAL_CONCENTRIC, 0.0, 0.0, 0.0],
        flags: [0.0; 4],
        focal_map: None,
    };

    /// Skia's `SkConicalGradient` decomposition, run once per draw.
    fn solve(center: Point, radius: f32, focus: Option<FocalCircle>) -> Self {
        // Two different epsilons on purpose, following Impeller: case
        // SELECTION uses the looser `kEhCloseEnough` (conical_gradient_contents
        // .cc), because a separation just above the tight one produces a
        // near-singular 1/length map and fp32 noise with it. The tight
        // 1/4096 stays for the in-shader constants (gradient.glsl).
        const CASE_EPSILON: f32 = 1.0e-3;
        const NEARLY_ZERO: f32 = 1.0 / (1 << 12) as f32;
        let start = focus.unwrap_or(FocalCircle::point(center));
        let separation = (center.x - start.center.x).hypot(center.y - start.center.y);

        // Concentric circles need no focal machinery: t is just how far the
        // point sits between the two radii.
        if separation < CASE_EPSILON {
            if (radius - start.radius).abs() < CASE_EPSILON {
                return Self {
                    constants: [CONICAL_EMPTY, 0.0, 0.0, 0.0],
                    ..Self::UNUSED
                };
            }
            return Self {
                constants: [CONICAL_CONCENTRIC, start.radius, radius, 0.0],
                flags: [0.0; 4],
                focal_map: None,
            };
        }

        // Equal radii have no focal point at all — the circles sweep a strip
        // between their common tangents, and `focal` below would divide by
        // zero. Skia and Impeller both carve this out as its own case.
        if (radius - start.radius).abs() < CASE_EPSILON {
            let radius_in_unit_space = start.radius / separation;
            return Self {
                constants: [
                    CONICAL_STRIP,
                    radius_in_unit_space * radius_in_unit_space,
                    0.0,
                    0.0,
                ],
                flags: [0.0; 4],
                focal_map: Some(map_to_unit_x(start.center, center)),
            };
        }

        // Steps 1-2: the focal parameter, and the swap that keeps it finite
        // when the two radii are equal.
        let (mut first, mut second) = (start.center, center);
        let mut focal = start.radius / (start.radius - radius);
        let is_swapped = (focal - 1.0).abs() < NEARLY_ZERO;
        if is_swapped {
            std::mem::swap(&mut first, &mut second);
            focal = 0.0f32;
        }

        // Steps 3-4: map [focal centre, end centre] onto [(0,0), (1,0)], then
        // scale so the end circle becomes the unit circle.
        let focal_center = Point::new(
            first.x * (1.0 - focal) + second.x * focal,
            first.y * (1.0 - focal) + second.y * focal,
        );
        let radius_in_unit_space = (radius - start.radius).abs() / separation;
        let is_focal_on_circle = (radius_in_unit_space - 1.0).abs() < NEARLY_ZERO;
        let span = (1.0 - focal).abs();
        let (scale_x, scale_y) = if is_focal_on_circle {
            (span * 0.5, span * 0.5)
        } else {
            let squared = radius_in_unit_space * radius_in_unit_space;
            (
                span * radius_in_unit_space / (squared - 1.0),
                span / (squared - 1.0).abs().sqrt(),
            )
        };

        let is_well_behaved = !is_focal_on_circle && radius_in_unit_space > 1.0;
        Self {
            constants: [
                CONICAL_GENERAL,
                radius_in_unit_space,
                focal,
                (1.0 - focal).signum(),
            ],
            flags: [
                is_swapped as u32 as f32,
                is_focal_on_circle as u32 as f32,
                is_well_behaved as u32 as f32,
                0.0,
            ],
            focal_map: Some(scale_after(
                map_to_unit_x(focal_center, second),
                scale_x,
                scale_y,
            )),
        }
    }
}

/// Maps `[from, to]` onto `[(0, 0), (1, 0)]`.
fn map_to_unit_x(from: Point, to: Point) -> Matrix {
    let (dx, dy) = (to.x - from.x, to.y - from.y);
    let length = dx.hypot(dy);
    let (ux, uy) = (dx / length, dy / length);
    Matrix::from_affine(
        ux / length,
        -uy / length,
        uy / length,
        ux / length,
        -(ux * from.x + uy * from.y) / length,
        (uy * from.x - ux * from.y) / length,
    )
}

/// `scale(x, y) ∘ matrix` for an affine 2D matrix.
fn scale_after(matrix: Matrix, x: f32, y: f32) -> Matrix {
    let [a, b, c, d, tx, ty] = matrix.to_affine();
    Matrix::from_affine(a * x, b * y, c * x, d * y, tx * x, ty * y)
}

/// Gradient geometry + stops into the payload (see the WGSL layout
/// contract). A focal radial's fx/fy ride the two spare floats
/// (GEOM.w / MISC.w); focus == center encodes "classic". The INVERSE of
/// the shader's local matrix lands in PAYLOAD_LOCAL — fragments map draw
/// space into gradient space with it (identity for plain gradients; a
/// non-invertible matrix degenerates to a constant ramp sample, never UB).
/// `ramp_texels` = Some(N) when the stops ride a baked texture: the count
/// lane carries N for the fragment's half-texel mapping, and the uniform
/// stop arrays stay untouched (the texture IS the ramp).
/// A pattern evaluates in its own space like a gradient does, but its
/// payload is only that mapping: `local⁻¹` into pattern pixels, then the
/// reciprocal image size to reach uv. Tiling and filtering ride the sampler.
fn fill_pattern_payload(
    record: &mut UniformRecord,
    image: &Image,
    sampling: Sampling,
    local: &Matrix,
) {
    let size = image.size();
    record.set_payload(
        PAYLOAD_GEOM,
        [1.0 / size[0] as f32, 1.0 / size[1] as f32, 0.0, 0.0],
    );
    record.set_payload(PAYLOAD_DECAL, decal_flags(sampling));
    let inverse = local
        .invert()
        .unwrap_or(Matrix::from_affine(0.0, 0.0, 0.0, 0.0, 0.0, 0.0));
    let [a, b, c, d, tx, ty] = inverse.to_affine();
    record.set_payload(PAYLOAD_LOCAL, [a, b, c, d]);
    record.set_payload(PAYLOAD_LOCAL + 1, [tx, ty, 0.0, 0.0]);
}

fn fill_gradient_payload(record: &mut UniformRecord, shader: &Shader, ramp_texels: Option<u32>) {
    let (geom, angle, misc_w) = match shader {
        Shader::Linear { start, end, .. } => ([start.x, start.y, end.x, end.y], 0.0, 0.0),
        Shader::Radial {
            center,
            radius,
            focus,
            ..
        } => {
            let f = focus.map_or(*center, |circle| circle.center);
            ([center.x, center.y, *radius, f.x], 0.0, f.y)
        }
        Shader::Sweep {
            center,
            start_angle,
            ..
        } => ([center.x, center.y, 0.0, 0.0], *start_angle, 0.0),
        Shader::Image { .. } => unreachable!("patterns fill their own payload"),
    };
    record.set_payload(PAYLOAD_GEOM, geom);

    let (Shader::Linear { local, .. }
    | Shader::Radial { local, .. }
    | Shader::Sweep { local, .. }
    | Shader::Image { local, .. }) = shader;
    let mut inverse = local
        .invert()
        .unwrap_or(Matrix::from_affine(0.0, 0.0, 0.0, 0.0, 0.0, 0.0));

    // A two-point conical gradient is solved in a space where the focal
    // point sits at the origin and the end circle is the unit circle. That
    // mapping is constant per draw, so it folds into the inverse local
    // matrix here and the fragment only runs the per-pixel half.
    let conical = match shader {
        Shader::Radial {
            center,
            radius,
            focus,
            ..
        } => ConicalSetup::solve(*center, *radius, *focus),
        _ => ConicalSetup::UNUSED,
    };
    if let Some(focal_map) = conical.focal_map {
        inverse = focal_map.then(&inverse);
    }
    record.set_payload(PAYLOAD_CONICAL, conical.constants);
    record.set_payload(PAYLOAD_CONICAL_FLAGS, conical.flags);

    // Gradient locals are affine by construction — the 2D block is exact.
    let [a, b, c, d, tx, ty] = inverse.to_affine();
    record.set_payload(PAYLOAD_LOCAL, [a, b, c, d]);
    record.set_payload(PAYLOAD_LOCAL + 1, [tx, ty, 0.0, 0.0]);

    let spread = match shader {
        Shader::Linear { spread, .. } | Shader::Radial { spread, .. } => *spread,
        // Sweep is periodic by nature; patterns tile through the sampler.
        Shader::Sweep { .. } | Shader::Image { .. } => SpreadMode::Pad,
    };
    let stops = shader.stops();
    let count = stops.len().min(MAX_GRADIENT_STOPS);
    let count_lane = match ramp_texels {
        Some(texels) => texels as f32,
        None => count as f32,
    };
    record.set_payload(
        PAYLOAD_MISC,
        [count_lane, angle, spread as u8 as f32, misc_w],
    );
    if ramp_texels.is_some() {
        return;
    }
    let mut offsets = [0.0f32; MAX_GRADIENT_STOPS];
    for (i, stop) in stops.iter().take(count).enumerate() {
        offsets[i] = stop.offset;
        record.set_payload(PAYLOAD_COLORS + i, stop.color.components());
    }
    record.set_payload(
        PAYLOAD_OFFSETS,
        [offsets[0], offsets[1], offsets[2], offsets[3]],
    );
    record.set_payload(
        PAYLOAD_OFFSETS + 1,
        [offsets[4], offsets[5], offsets[6], offsets[7]],
    );
}

/// uv = local × scale + offset, mapping `dst` (local px) onto `src`
/// (texture px, normalized) — out-of-range uv is the sampler's business.
fn uv_mapping(image: &Image, src: &Rect, dst: &Rect) -> [f32; 4] {
    let (tw, th) = (image.width(), image.height());
    let sx = src.width / (dst.width * tw);
    let sy = src.height / (dst.height * th);
    [sx, sy, src.x / tw - dst.x * sx, src.y / th - dst.y * sy]
}

/// uv over a full texture stretched across `rect` (layer composites).
/// A layer's texture extent: its fractional rect, ceil'd. The ONE formula —
/// allocation and sampling must agree or edge texels stretch.
fn layer_texture_size(rect: &Rect) -> [u32; 2] {
    [
        rect.width.ceil().max(1.0) as u32,
        rect.height.ceil().max(1.0) as u32,
    ]
}

fn full_rect_uv(rect: &Rect) -> [f32; 4] {
    let sx = 1.0 / rect.width;
    let sy = 1.0 / rect.height;
    [sx, sy, -rect.x * sx, -rect.y * sy]
}

/// Impeller's CalculateScale: σ ≤ 4 runs full resolution; past
/// that, downsample until the effective σ is ~4. No floor — a fixed one
/// (the old 1/16) truncated the gaussian tail past device σ ≈ 300 (boxy,
/// bright-edged); the downsample chain halves per pass, so depth stays
/// log₂ regardless.
fn blur_scale(sigma: f32) -> f32 {
    if sigma <= 4.0 {
        return 1.0;
    }
    (4.0 / sigma).log2().round().exp2()
}

/// Emoji rasters cap here in the outline tier (they have no outlines);
/// beyond it the bitmap upscales — Skia clamps the same way for glyphs too
/// big for the atlas.
const MAX_COLOR_GLYPH_PX: f32 = 256.0;

/// Impeller's mask-tier scale quantization (text_frame.cc
/// RoundScaledFontSize): 1/200 steps, clamped so a glyph always fits the
/// atlas — floating noise dedupes, real zoom re-rasters.
fn quantize_scale(scale: f32) -> f32 {
    ((scale * 200.0).round() / 200.0).clamp(1.0 / 200.0, 48.0)
}

/// Snap x to the pixel grid + a quarter-px phase (Skia's 2-bit subpixel
/// ids / Impeller's ComputeFractionalPosition): the raster carries the
/// fraction, the quad sits on the integer.
fn snap_quarter(x: f32) -> (f32, u8) {
    let quarters = (x * 4.0).round();
    let base = (quarters * 0.25).floor();
    let phase = (quarters - base * 4.0) as u8 % 4;
    (base, phase)
}

/// Scale + translate only: device-space snapping is meaningful. Rotation
/// and flips take the transformed-quad route.
fn is_axis_aligned(t: &Matrix) -> bool {
    t.kind() == valo_geometry::MatrixKind::AxisAligned
}

fn is_uniform_axis_aligned(transform: &Matrix) -> bool {
    if !is_axis_aligned(transform) {
        return false;
    }
    let [scale_x, _, _, scale_y, ..] = transform.to_affine();
    (scale_x - scale_y).abs() <= 1e-6 * scale_x.max(scale_y).max(1.0)
}

fn text_mode(page: PageRef, coverage: Coverage) -> TextMode {
    match (page.color, coverage) {
        (true, _) => TextMode::Color,
        (false, Coverage::Sdf) => TextMode::Sdf,
        (false, _) => TextMode::Mask,
    }
}

/// Batch quads per (mode, atlas page) in first-seen order — deterministic
/// step emission, one draw per page.
fn batch_for(
    batches: &mut Vec<((TextMode, PageRef), Vec<f32>)>,
    mode: TextMode,
    page: PageRef,
) -> &mut Vec<f32> {
    let key = (mode, page);
    if let Some(at) = batches.iter().position(|(k, _)| *k == key) {
        return &mut batches[at].1;
    }
    batches.push((key, Vec::new()));
    &mut batches.last_mut().expect("just pushed").1
}

/// A stroked mask still has to fit an atlas cell, and the miter reach is
/// unbounded in the paint. Past this the run keeps taking the outline path,
/// where geometry has no size ceiling — the same escape the huge-text tier
/// already is.
const MAX_STROKED_MASK_PX: f32 = 1024.0;

/// Which tier a run lands in. The mask tier carries what its entries are
/// keyed on, plus the alpha a floored hairline gives back.
enum GlyphTier {
    Mask { coverage: Coverage, alpha: f32 },
    Sdf,
    Outline,
}

/// Skia's tier dispatch (SubRunControl.cpp), plus the stroke. A stroked run
/// is an ordinary mask-tier run because the rasterizer strokes the outline
/// before rasterizing it — but it never reaches the SDF tier, whose field
/// measures distance from a FILL boundary, and Impeller's stroked glyphs go
/// to the regular atlas for exactly that reason.
fn glyph_tier(tiers: TextTiers, paint: &Paint, scale: f32, device_px: f32) -> GlyphTier {
    if device_px >= tiers.path_min {
        return GlyphTier::Outline;
    }
    let stroke = match &paint.style {
        PaintStyle::Fill if device_px >= tiers.sdf_min => return GlyphTier::Sdf,
        PaintStyle::Fill => {
            return GlyphTier::Mask {
                coverage: Coverage::Fill,
                alpha: 1.0,
            }
        }
        PaintStyle::Stroke(stroke) => stroke,
    };
    match atlas_stroke(stroke, scale, device_px) {
        Some((stroke, alpha)) => GlyphTier::Mask {
            coverage: Coverage::Stroke(stroke),
            alpha,
        },
        None => GlyphTier::Outline,
    }
}

/// The mask tier's form of a stroke, in the raster's own pixels, with the
/// alpha its floored width owes back. `None` keeps the run on the outline
/// path: a dash is a variable-length pattern that a fixed-size atlas key
/// cannot hold, and a stroke whose miter can reach further than a cell has
/// nowhere to be packed.
fn atlas_stroke(stroke: &Stroke, scale: f32, device_px: f32) -> Option<(GlyphStroke, f32)> {
    if stroke.dash.is_some() {
        return None;
    }
    let device_width = stroke.width * scale;
    let width = device_width.max(1.0);
    // Worst case a join can reach past the glyph's own box, on every side.
    let reach = width * 0.5 * stroke.miter_limit.max(1.0);
    if 2.0 * (device_px + reach) > MAX_STROKED_MASK_PX {
        return None;
    }
    Some((
        GlyphStroke {
            width,
            cap: stroke.cap,
            join: stroke.join,
            miter_limit: stroke.miter_limit,
        },
        subpixel_stroke_alpha(device_width),
    ))
}

/// Skia's SDF strike buckets (SubRunControl::getSDFFont): raster at the
/// bucket, reuse while the device size stays within it.
fn sdf_bucket(device_px: f32) -> f32 {
    let buckets = crate::glyphs::SDF_BUCKETS;
    for bucket in buckets {
        if device_px <= bucket {
            return bucket;
        }
    }
    buckets[buckets.len() - 1]
}

/// One glyph quad at origin (gx, gy): placement hangs off it (left/top,
/// y-up), uv from the atlas slot. `scale` maps raster px → quad units
/// (1.0 in the device-snapped tier: texels 1:1).
fn push_glyph_quad(
    out: &mut Vec<f32>,
    gx: f32,
    gy: f32,
    entry: &crate::glyphs::AtlasGlyph,
    scale: f32,
) {
    let x0 = gx + entry.left * scale;
    let y0 = gy - entry.top * scale;
    let x1 = x0 + entry.width * scale;
    let y1 = y0 + entry.height * scale;
    let [u0, v0, u1, v1] = entry.uv;
    let quad = [
        [x0, y0, u0, v0],
        [x1, y0, u1, v0],
        [x0, y1, u0, v1],
        [x1, y0, u1, v0],
        [x1, y1, u1, v1],
        [x0, y1, u0, v1],
    ];
    for v in quad {
        out.extend_from_slice(&v);
    }
}

/// Filter targets snap up to the pool bucket so chains share textures.
fn filter_bucket(px: f32) -> u32 {
    (px.ceil().max(1.0) as u32).div_ceil(FILTER_SIZE_BUCKET) * FILTER_SIZE_BUCKET
}

/// A blur pass's target holds exactly the texels it writes, the way Impeller
/// allocates `subpass_size`.
///
/// Bucketing leaves cleared texels beside the used corner, and every read of
/// a blur target runs past that corner: the composite's linear upscale reaches
/// half a texel beyond the last real one, and `fs_blur`'s own ±radius taps
/// reach much further. Clamp-to-edge answers those reads on the left and top,
/// cleared gutter answers them on the right and bottom. So the far borders
/// faded where the near ones held, AND the blur came out slightly too strong,
/// since half its out-of-range taps returned transparent instead of the edge.
/// At σ 64 — one blur texel per sixteen output pixels — that was a pixel of
/// centroid and a whole level of amplitude away from Chrome.
///
/// The cost, paid knowingly: `filter_bucket` existed so a blur chain whose
/// size drifts frame to frame keeps reusing one pooled texture. Exact extents
/// give that up, so an animating blur can allocate a fresh target per frame
/// (bounded by the pool's eviction). Nothing measures that yet. If it ever
/// bites, measure a real animated-blur scene first — rounding some passes back
/// up is a guess about which reads are safe, and the reads are the whole
/// problem.
fn exact_extent(px: f32) -> u32 {
    px.ceil().max(1.0) as u32
}

/// local (0..work px) → uv spanning `region` inside a `source_size` texture
/// (the blur chain's first read; work < region px when downsampling).
fn source_region_uv(region: &Rect, source_size: [u32; 2], work: [f32; 2]) -> [f32; 4] {
    let sw = source_size[0] as f32;
    let sh = source_size[1] as f32;
    [
        region.width / (sw * work[0]),
        region.height / (sh * work[1]),
        region.x / sw,
        region.y / sh,
    ]
}

/// local (0..work px) → uv in a bucketed intermediate whose used corner
/// starts at the origin.
/// Per-axis decal flags: 1 where the fragment must cut off outside the image,
/// 0 where the sampler's own address mode already produces the right pixels.
/// Every image-sampling fragment reads these from `PAYLOAD_DECAL`, so a
/// pattern and a direct `drawImage` honour `TileMode::Decal` identically.
fn decal_flags(sampling: Sampling) -> [f32; 4] {
    [
        f32::from(sampling.tile_x == TileMode::Decal),
        f32::from(sampling.tile_y == TileMode::Decal),
        0.0,
        0.0,
    ]
}

fn corner_uv(bucket: [f32; 2]) -> [f32; 4] {
    [1.0 / bucket[0], 1.0 / bucket[1], 0.0, 0.0]
}

/// Map a filter pass's local quad (0..`work`) onto the whole used corner of
/// its source, whose content ends at `source_uv_max`.
///
/// The TARGET size belongs in this mapping, not just the source's: a
/// downsample pass draws a smaller quad and still has to cover the entire
/// source. Dividing by the source's own extent instead reads only the
/// top-left `work/source` fraction of it, which is why a blur whose σ crosses
/// the downsample threshold used to lose its left and top spread.
fn resample_uv(source_uv_max: [f32; 2], work: [f32; 2]) -> [f32; 4] {
    [
        source_uv_max[0] / work[0],
        source_uv_max[1] / work[1],
        0.0,
        0.0,
    ]
}

fn image_filter_stages<'a>(filter: &'a ImageFilter, stages: &mut Vec<&'a ImageFilter>) {
    match filter {
        ImageFilter::Compose { outer, inner } => {
            image_filter_stages(inner, stages);
            image_filter_stages(outer, stages);
        }
        stage => stages.push(stage),
    }
}

/// A local-space blur σ mapped onto the device axes, as Impeller's
/// `GaussianBlurFilterContents` does it: transform σ as a VECTOR by the effect
/// transform's basis, then take the component-wise absolute value.
///
/// Collapsing the basis to its two axis LENGTHS instead would lose the
/// rotation — a quarter turn has unit-length axes, so an anisotropic σ would
/// pass through unswapped and blur along the wrong axis.
///
/// The blur stays axis-aligned in device space for angles in between, which is
/// Impeller's approximation too.
fn device_sigma(basis: [f32; 4], sigma_x: f32, sigma_y: f32) -> [f32; 2] {
    let [a, b, c, d] = basis;
    [
        (sigma_x * a + sigma_y * c).abs(),
        (sigma_x * b + sigma_y * d).abs(),
    ]
}

/// The same mapping as Skia's, for the filters whose reference is Skia rather
/// than Impeller. `SkImageFilters::Blur` carries σ as an `SkSize` and maps
/// each axis BASIS separately, taking its length
/// (`SkImageFilterTypes.cpp`'s `mapSize` for non-scale/translate matrices).
///
/// The two rules disagree exactly where it is visible: under a 45° rotation
/// Impeller's vector rule sends an isotropic σ of (10, 10) to (0, 14.14) —
/// an anisotropic blur that no longer looks round — while Skia's leaves it
/// at (10, 10). Drop shadow follows Skia because
/// `SkImageFilters::DropShadow` is what CSS `drop-shadow()` lowers to; plain
/// [`ImageFilter::Blur`] keeps Impeller's rule, which is what its own
/// documentation and goldens are written against.
fn skia_sigma(basis: [f32; 4], sigma_x: f32, sigma_y: f32) -> [f32; 2] {
    let [a, b, c, d] = basis;
    [sigma_x * a.hypot(b), sigma_y * c.hypot(d)]
}

/// A local-space filter offset mapped onto the device axes. Sign-preserving,
/// unlike [`device_sigma`] — a shadow that leans right has to keep leaning
/// right after the basis mirrors or rotates it.
fn device_offset(basis: [f32; 4], offset: Point) -> [f32; 2] {
    let [a, b, c, d] = basis;
    [offset.x * a + offset.y * c, offset.x * b + offset.y * d]
}

/// local (absolute px) → uv into a blurred texture holding `region`, whose
/// used corner ends at `uv_max`.
fn region_uv(region: &Rect, uv_max: [f32; 2]) -> [f32; 4] {
    let sx = uv_max[0] / region.width.max(1e-6);
    let sy = uv_max[1] / region.height.max(1e-6);
    [sx, sy, -region.x * sx, -region.y * sy]
}

/// Covers every pixel it touches: full alpha, a blend that ignores dst,
/// no soft coverage. (Images and layer composites stay out — their texel
/// alpha is unknowable here.)
fn is_opaque_paint(paint: &Paint) -> bool {
    let solid_blend = matches!(paint.blend_mode, BlendMode::SrcOver | BlendMode::Src);
    solid_blend
        && paint.mask_blur.is_none()
        && paint.effective_image_filter().is_none()
        && paint.color.a >= 1.0
        && paint.shader.as_ref().is_none_or(shader_opaque)
}

fn shader_opaque(shader: &Shader) -> bool {
    // A two-point conical gradient with a real start circle does not cover
    // the plane: outside its cone — the whole region beyond the tangents in
    // the strip case — nothing is painted at all. Opaque promotion would
    // turn those pixels into replaced black, so a gradient that can leave
    // gaps never qualifies, however opaque its stops are. Nested circles do
    // cover everything, but proving that per draw buys back only a depth
    // optimisation, so this stays conservative.
    if let Shader::Radial {
        center,
        focus: Some(circle),
        ..
    } = shader
    {
        // A start RADIUS, or a focal point away from the centre, can leave
        // pixels behind the focal cone uncovered — the shader returns
        // transparent there, so the draw cannot claim to fill its geometry.
        if circle.radius > 0.0 || circle.center != *center {
            return false;
        }
    }
    let stops = match shader {
        Shader::Linear { stops, .. }
        | Shader::Radial { stops, .. }
        | Shader::Sweep { stops, .. } => stops,
        // A pattern's alpha lives in texels nobody has read at plan time.
        Shader::Image { .. } => return false,
    };
    stops.iter().all(|stop| stop.color.a >= 1.0)
}

/// Impeller's DrawOrderResolver, as a pure pass over one
/// segment: fans glue to the step that follows them (a draw unit), clip
/// units are BARRIERS, and between barriers opaque units draw first,
/// front-to-back — painter order among the rest is untouched. Hoisting
/// never crosses a barrier (a clip ceiling must be in the depth buffer
/// before the draws it scopes), which is slightly more conservative than
/// Impeller's resolver.
fn reorder_segment(steps: Vec<Step>, hoisted: &mut u32) -> Vec<Step> {
    let mut out = Vec::with_capacity(steps.len());
    let mut chunk: Vec<Vec<Step>> = Vec::new(); // draw units between barriers
    let mut unit: Vec<Step> = Vec::new();
    for step in steps {
        let fan = matches!(step.key.kind, PipelineKind::StencilFan { .. });
        let barrier = matches!(step.key.kind, PipelineKind::ClipCover { .. });
        unit.push(step);
        if fan {
            continue; // the unit closes at its cover/draw
        }
        if barrier {
            flush_chunk(&mut chunk, &mut out, hoisted);
            out.append(&mut unit);
        } else {
            chunk.push(std::mem::take(&mut unit));
        }
    }
    chunk.push(unit); // a trailing fan-only unit can't exist; this may be empty
    flush_chunk(&mut chunk, &mut out, hoisted);
    out
}

/// Emit one barrier-free chunk: opaque units first (z descending = front to
/// back), everything else in painter order.
fn flush_chunk(chunk: &mut Vec<Vec<Step>>, out: &mut Vec<Step>, hoisted: &mut u32) {
    let is_opaque = |unit: &Vec<Step>| {
        unit.last().is_some_and(|s| {
            matches!(
                s.key.kind,
                PipelineKind::OpaqueDraw(_) | PipelineKind::OpaqueCover(_)
            )
        })
    };
    let mut seen_blended = false;
    let mut opaque: Vec<Vec<Step>> = Vec::new();
    let mut blended: Vec<Vec<Step>> = Vec::new();
    for unit in chunk.drain(..) {
        if is_opaque(&unit) {
            if seen_blended {
                *hoisted += 1; // drawn out of painter order
            }
            opaque.push(unit);
        } else if !unit.is_empty() {
            seen_blended = true;
            blended.push(unit);
        }
    }
    opaque.sort_by(|a, b| {
        let za = a.last().map_or(0.0, |s| s.sort_z);
        let zb = b.last().map_or(0.0, |s| s.sort_z);
        zb.total_cmp(&za)
    });
    out.extend(opaque.into_iter().flatten());
    out.extend(blended.into_iter().flatten());
}

/// The in-layer version of a blurred draw: geometry and color only — blur
/// and blend ride the composite.
/// The paint the inner draw of an effect layer uses: the effects moved to
/// the layer, and the blend deferred to the composite.
fn plain(paint: &Paint) -> Paint {
    Paint {
        blend_mode: BlendMode::SrcOver,
        mask_blur: None,
        color_filter: None,
        image_filter: None,
        ..paint.clone()
    }
}

/// A solid or gradient paint absorbs its colour filter on the CPU, matching
/// Impeller's `Contents::ApplyColorFilter`. Image patterns return `None` and
/// become cached filtered-source textures in `prepared_paint`.
fn folded_paint(paint: &Paint) -> Option<Paint> {
    let filter = paint.color_filter?;
    let mut folded = paint.clone();
    match &mut folded.shader {
        // A gradient absorbs the filter into its stops.
        Some(shader) => {
            if !shader.fold_color_filter(&filter) {
                return None;
            }
        }
        // A solid paint absorbs it into the colour itself.
        None => folded.color = filter.folded_into(paint.color)?,
    }
    folded.color_filter = None;
    Some(folded)
}

/// Does this paint need its own layer for the renderer to apply its effects?
fn needs_effect_layer(paint: &Paint) -> bool {
    paint.mask_blur.is_some()
        || paint.color_filter.is_some()
        || paint.effective_image_filter().is_some()
}

/// The frame-local integer pixel region under `coverage` (absolute replay
/// coords), rounded outward and clamped — `None` when nothing visible needs
/// copying. Fragments only exist inside their draw's bounds, so this region
/// is every texel a dst read can touch.
fn snapshot_region(coverage: &Rect, origin: Point, size: [u32; 2]) -> Option<([u32; 2], [u32; 2])> {
    let x0 = (coverage.x - origin.x).floor().max(0.0) as u32;
    let y0 = (coverage.y - origin.y).floor().max(0.0) as u32;
    let x1 = ((coverage.x + coverage.width - origin.x).ceil().max(0.0) as u32).min(size[0]);
    let y1 = ((coverage.y + coverage.height - origin.y).ceil().max(0.0) as u32).min(size[1]);
    (x1 > x0 && y1 > y0).then_some(([x0, y0], [x1 - x0, y1 - y0]))
}

fn fan_kind(rule: FillRule) -> PipelineKind {
    PipelineKind::StencilFan {
        even_odd: rule == FillRule::EvenOdd,
    }
}

/// Unit quad → this rect, as a transform (bakes geometry into the MVP).
fn rect_to_unit(r: &Rect) -> Matrix {
    Matrix::from_affine(r.width, 0.0, 0.0, r.height, r.x, r.y)
}

/// Triangle-list fan per contour: (p0, pi, pi+1). Overlap and orientation are
/// fine — the stencil winding sorts coverage out.
fn fan_vertices(contours: &[valo_geometry::Contour]) -> Vec<f32> {
    let triangles: usize = contours
        .iter()
        .map(|c| c.points.len().saturating_sub(2))
        .sum();
    let mut out = Vec::with_capacity(triangles * 6);
    for contour in contours {
        let contour = &contour.points;
        let p0 = contour[0];
        for pair in contour[1..].windows(2) {
            out.extend_from_slice(&[p0.x, p0.y, pair[0].x, pair[0].y, pair[1].x, pair[1].y]);
        }
    }
    out
}

/// Model → column-major mat4 MVP: y-down ortho (x: [0,w]→[-1,1],
/// y: [0,h]→[1,-1]) with the draw's depth slot folded in. The z row is
/// REPLACED by z × (w row): after the hardware divide every fragment lands
/// exactly at the draw's slot, perspective or not — the model's own z
/// output is meaningless for 2D content (Impeller's convention).
#[rustfmt::skip]
fn ortho_mvp(m: &Matrix, size: [u32; 2], z: f32) -> [f32; 16] {
    let (w, h) = (size[0] as f32, size[1] as f32);
    let projection = glam::Mat4::from_cols_array(&[
        2.0 / w, 0.0,      0.0, 0.0,
        0.0,     -2.0 / h, 0.0, 0.0,
        0.0,     0.0,      1.0, 0.0,
        -1.0,    1.0,      0.0, 1.0,
    ]);
    let mut mvp = projection * m.to_mat4();
    mvp.x_axis.z = z * mvp.x_axis.w;
    mvp.y_axis.z = z * mvp.y_axis.w;
    mvp.z_axis.z = z * mvp.z_axis.w;
    mvp.w_axis.z = z * mvp.w_axis.w;
    mvp.to_cols_array()
}

pub(crate) fn linear_sampler(device: &wgpu::Device) -> wgpu::Sampler {
    device.create_sampler(&wgpu::SamplerDescriptor {
        label: Some("valo.plan"),
        mag_filter: wgpu::FilterMode::Linear,
        min_filter: wgpu::FilterMode::Linear,
        ..Default::default()
    })
}

#[cfg(test)]
mod tests {
    use super::{device_sigma, is_uniform_axis_aligned, skia_sigma, stroke_alpha_coverage};
    use valo_geometry::Matrix;

    fn basis_of(matrix: &Matrix) -> [f32; 4] {
        let [a, b, c, d, ..] = matrix.to_affine();
        [a, b, c, d]
    }

    fn assert_sigma(actual: [f32; 2], expected: [f32; 2]) {
        assert!(
            (actual[0] - expected[0]).abs() < 1e-4 && (actual[1] - expected[1]).abs() < 1e-4,
            "sigma {actual:?} != {expected:?}"
        );
    }

    /// Drop shadow follows `SkImageFilters::Blur`'s `SkSize` mapping, so a
    /// rotation must leave an isotropic σ isotropic. Impeller's vector rule
    /// turns the same input into a directional smear.
    #[test]
    fn skia_sigma_keeps_a_rotated_blur_round() {
        let basis = basis_of(&Matrix::rotation(std::f32::consts::FRAC_PI_4));
        assert_sigma(skia_sigma(basis, 10.0, 10.0), [10.0, 10.0]);
        assert_sigma(device_sigma(basis, 10.0, 10.0), [0.0, 14.142136]);
    }

    #[test]
    fn skia_sigma_still_scales_each_axis() {
        let basis = basis_of(&Matrix::scale(2.0, 3.0));
        assert_sigma(skia_sigma(basis, 4.0, 5.0), [8.0, 15.0]);
    }

    #[test]
    fn device_sigma_scales_each_axis() {
        let basis = basis_of(&Matrix::scale(2.0, 3.0));
        assert_sigma(device_sigma(basis, 4.0, 5.0), [8.0, 15.0]);
    }

    /// A quarter turn maps local x onto device y. Reducing the basis to its
    /// axis LENGTHS would give [1, 1] here and leave σ unswapped — the bug
    /// this function exists to prevent.
    #[test]
    fn device_sigma_swaps_axes_under_a_quarter_turn() {
        let basis = basis_of(&Matrix::rotation(std::f32::consts::FRAC_PI_2));
        assert_sigma(device_sigma(basis, 12.0, 3.0), [3.0, 12.0]);
    }

    /// Impeller takes the component-wise absolute value, so a half turn is
    /// indistinguishable from no rotation at all.
    #[test]
    fn device_sigma_is_sign_agnostic() {
        let basis = basis_of(&Matrix::rotation(std::f32::consts::PI));
        assert_sigma(device_sigma(basis, 7.0, 2.0), [7.0, 2.0]);
    }

    /// Rotation composed with non-uniform scale: each device axis takes a
    /// contribution from BOTH local sigmas, which is the case an axis-length
    /// reduction cannot express at any angle.
    #[test]
    fn device_sigma_mixes_axes_under_rotation_and_scale() {
        let matrix = Matrix::scale(2.0, 3.0).then(&Matrix::rotation(std::f32::consts::FRAC_PI_4));
        let [a, b, c, d, ..] = matrix.to_affine();
        let expected = [(6.0 * a + 4.0 * c).abs(), (6.0 * b + 4.0 * d).abs()];
        assert_sigma(device_sigma(basis_of(&matrix), 6.0, 4.0), expected);
        // Both axes genuinely mix — a degenerate basis would pass vacuously.
        assert!(expected[0] > 1.0 && expected[1] > 1.0);
    }

    /// A mirror has a negative determinant; `.Abs()` makes it equivalent to
    /// its unmirrored twin.
    #[test]
    fn device_sigma_ignores_a_mirror() {
        let mirrored = basis_of(&Matrix::scale(-2.0, 3.0));
        assert_sigma(device_sigma(mirrored, 4.0, 5.0), [8.0, 15.0]);
    }

    /// Two blurs under a 45° rotation. Transforming each stage and THEN
    /// combining leaves both device axes equal; combining in local space
    /// first and rotating the total collapses σx to zero and piles
    /// everything onto y — the divergence from Impeller's per-filter
    /// `CalculateBlurInfo` that this ordering exists to avoid.
    #[test]
    fn composed_blurs_transform_before_they_combine() {
        let basis = basis_of(&Matrix::rotation(std::f32::consts::FRAC_PI_4));
        let stages = [(10.0f32, 0.0f32), (0.0f32, 10.0f32)];

        let (mut x_squared, mut y_squared) = (0.0, 0.0);
        for (sigma_x, sigma_y) in stages {
            let [x, y] = device_sigma(basis, sigma_x, sigma_y);
            x_squared += x * x;
            y_squared += y * y;
        }
        assert_sigma([x_squared.sqrt(), y_squared.sqrt()], [10.0, 10.0]);

        let combined = device_sigma(basis, 10.0, 10.0);
        assert!(
            combined[0] < 0.001 && combined[1] > 14.0,
            "combining first must be the WRONG answer, got {combined:?}"
        );
    }

    #[test]
    fn hairline_coverage_matches_impeller() {
        assert_eq!(stroke_alpha_coverage(&Matrix::IDENTITY, 0.0), 1.0);
        assert_eq!(stroke_alpha_coverage(&Matrix::IDENTITY, 0.25), 0.5);
        assert_eq!(stroke_alpha_coverage(&Matrix::IDENTITY, 0.5), 1.0);
        assert_eq!(stroke_alpha_coverage(&Matrix::scale(2.0, 2.0), 0.25), 1.0);
    }

    #[test]
    fn snapped_text_requires_uniform_scale() {
        assert!(is_uniform_axis_aligned(&Matrix::scale(0.5, 0.5)));
        assert!(!is_uniform_axis_aligned(&Matrix::scale(0.5, 1.0)));
        assert!(!is_uniform_axis_aligned(&Matrix::rotation(0.1)));
    }
}
