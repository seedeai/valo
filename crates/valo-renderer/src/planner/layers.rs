//! The open-target stack. A [`PassFrame`] is one render target being filled:
//! the main target at the bottom, one frame per materialized layer above it.
//! This module owns the layer lifecycle — open (or elide, or skip), fill,
//! close, composite — and the group-alpha stack for elided opacity layers.
//!
//! Replay state never lives here: slot rebasing and the outer group-alpha
//! stack ride the scope entry in `replay`, so a `PassFrame` is purely a
//! render target.

use rustc_hash::FxHashMap;
use valo_dl::{BlendMode, DisplayList, MaskKind, Paint};
use valo_geometry::{Color, Matrix, Point, Rect};

use crate::frame::{PassColor, Step, TextureCopy};
use crate::pipelines::{Frag, PipelineKind};
use crate::raster::FillTarget;
use crate::renderer::RenderTarget;

use super::emit::{alpha_tint, PAYLOAD_GEOM, PAYLOAD_MISC};
use super::filters::{region_uv, LayerEffects, SharedBlur};
use super::Planner;

/// `BackdropRequest` is a recorded backdrop's facts, resolved by replay.
///
/// Replay validates the shared key against the list's backdrop groups
/// before the layer opens, so the seed logic never re-checks σ agreement.
pub(super) struct BackdropRequest {
    /// Blur σ in local units; the save-point transform scales it.
    pub sigma_local: f32,
    /// The shared key, already validated — cleared when the group's tiles
    /// disagree on σ.
    pub key: Option<u64>,
    /// The keyed group's union bounds, list-root space.
    pub group_bounds: Option<Rect>,
}

/// `BackdropSeed` is the blurred parent region a backdrop layer opens with.
///
/// Drawn as the layer's first step — the glass every child paints over.
struct BackdropSeed {
    /// The finished blur.
    view: wgpu::TextureView,
    /// The region the blur covers, absolute replay coords.
    region: Rect,
    /// The used corner of the (possibly downsampled) blur texture.
    uv_max: [f32; 2],
}

/// `PassFrame` is one open render target.
///
/// Coordinates are absolute replay coordinates — the transform stack is
/// never rebased; a layer's `origin` is subtracted at MVP time instead,
/// so children land in layer pixels.
pub(super) struct PassFrame {
    pub color: PassColor,
    pub depth: wgpu::TextureView,
    /// The resolved texture snapshots copy from (`break_pass`).
    pub src_texture: wgpu::Texture,
    pub size: [u32; 2],
    /// `Some` clears on this target's first segment; `None` loads existing.
    pub clear: Option<Color>,
    pub first_segment_emitted: bool,
    /// Index of this target's most recent segment in the plan — `Some` marks
    /// a resumed target whose earlier segment must store its attachments.
    pub last_pass: Option<usize>,
    pub steps: Vec<Step>,
    pub pre_copies: Vec<TextureCopy>,
    /// Children's cull rect, absolute replay coords.
    pub cull_rect: Rect,
    /// Depth denominator: the slot span this target hosts, keeping every z
    /// strictly below 1.
    pub z_denom: f32,
    pub origin: Point,
    /// Cleared targets render into tile-only MSAA scratch; a dst-read break
    /// forces a swap to persistent attachments.
    pub transient: bool,
    /// `Some` on layer frames: everything the composite-on-close needs.
    pub layer: Option<LayerInfo>,
}

impl PassFrame {
    /// `main` builds the bottom frame: the caller's target.
    pub fn main(
        color: PassColor,
        depth: wgpu::TextureView,
        target: &RenderTarget,
        dl: &DisplayList,
        transient: bool,
    ) -> Self {
        Self {
            color,
            depth,
            src_texture: target.texture.clone(),
            size: target.size,
            clear: target.clear,
            first_segment_emitted: false,
            last_pass: None,
            steps: Vec::with_capacity(dl.draw_count() as usize * 2),
            pre_copies: Vec::new(),
            cull_rect: Rect::new(0.0, 0.0, target.size[0] as f32, target.size[1] as f32),
            z_denom: (dl.depth_slots() + 1) as f32,
            origin: Point::ZERO,
            transient,
            layer: None,
        }
    }
}

/// `LayerInfo` carries what closing a layer needs to draw it into its
/// parent.
pub(super) struct LayerInfo {
    /// The layer's region in parent coords (also its pixel extent).
    pub rect: Rect,
    /// The composite paint; group alpha from an elided enclosing scope was
    /// already folded into its color at open.
    pub paint: Paint,
    pub mask_composite: Option<MaskKind>,
    /// The composite's absolute z, taken from the recorded composite slot
    /// while the PARENT's slot base was still active.
    pub composite_z: f32,
    pub resolve: wgpu::TextureView,
    /// The paint's composite-time filter recipe (blur, colour filter,
    /// image filter) — `filters` runs it in [`Planner::composite_source`].
    pub effects: LayerEffects,
}

/// `ResolvedLayer` is one `SaveLayer` op pinned to one replay.
///
/// The op is written once and replayed anywhere — nested inside another
/// list, or into a cache texture, at whatever depth the walk has reached —
/// so it carries a slot NUMBER rather than a depth, and a backdrop key it
/// has no standing to validate. The walk fills both in (`composite_z`, a
/// checked `backdrop`) and hands the whole op over, so opening a layer
/// computes nothing. Skia passes the same set as `SkCanvas::SaveLayerRec`.
pub(super) struct ResolvedLayer<'a> {
    /// Configures the composite draw that puts the finished layer into its
    /// parent — blend mode and filters treat the layer as one image.
    ///
    /// Read at OPEN, not at close: the children take the group-alpha stack
    /// away and rebase the depth line, so by close the parent context this
    /// depends on is gone.
    pub paint: &'a Paint,
    /// Set = the layer's texture is coverage, not content.
    ///
    /// The composite turns its pixels into a multiplier on the parent
    /// (DstIn over the whole enclosing extent), so everything the mask
    /// does not cover disappears.
    pub mask: Option<MaskKind>,
    /// Union of the children's ink, list-root space, already cropped by
    /// the clip stack and any bounds hint. Sizes the layer's texture.
    pub bounds: &'a Rect,
    /// The depth line's position when the scope opened. Children run from
    /// here to `composite_slot`, and the layer's pass rebases against it.
    pub base_slot: u32,
    /// Where the composite draw sits, after the children's span.
    pub composite_slot: u32,
    /// The composite's depth, taken while the PARENT's slot base was still
    /// active — it draws in the parent, not in the layer.
    pub composite_z: f32,
    /// The children turned out alpha-linear and pairwise disjoint, so
    /// nothing overlaps for the group's alpha to blend twice: it can ride
    /// each child's own tint and the texture disappears entirely.
    pub can_elide: bool,
    /// Set = the layer opens pre-filled with the blurred scene beneath it.
    pub backdrop: Option<BackdropRequest>,
}

/// `Opened` is `open_layer`'s verdict, which replay turns into the matching
/// restore action.
pub(super) enum Opened {
    /// Nothing visible: skip the scope's ops entirely.
    Skip,
    /// The opacity shortcut: children draw in the parent with the group
    /// alpha on their tints; restore pops the alpha.
    Elided,
    /// A real offscreen: restore closes and composites it.
    Layer,
}

impl Planner<'_> {
    /// `open_layer` opens a recorded `SaveLayer`: skip (and erase, if it was
    /// a mask), elide into the parent, or push a new offscreen frame.
    ///
    /// Elision is the opacity peephole — children draw in the parent at
    /// their own slots with the group alpha on their tint. Depth does not
    /// change, which is what makes it safe. A mask whose bounds miss the
    /// cull rect is not "nothing": coverage is 0 everywhere, so the
    /// enclosing layer goes blank via [`Planner::erase_frame_alpha`].
    pub(super) fn open_layer(
        &mut self,
        base: &Matrix,
        effect_transform: &Matrix,
        layer: ResolvedLayer<'_>,
        shared_blurs: &mut FxHashMap<u64, SharedBlur>,
    ) -> Opened {
        let Some(rect) = self.layer_rect(base, layer.bounds) else {
            if layer.mask.is_some() {
                self.erase_frame_alpha(layer.composite_z);
            }
            return Opened::Skip;
        };
        if layer.can_elide {
            self.stats.layers_elided += 1;
            self.push_elision(layer.paint.color.a);
            return Opened::Elided;
        }
        self.stats.layers_rendered += 1;
        // The blurred parent is sampled BEFORE the layer's texture opens —
        // this ordering is the whole point of backdrop-as-a-layer-property:
        // the glass shows the real scene, not a fresh offscreen.
        let seed = layer.backdrop.map(|request| {
            self.backdrop_seed(base, effect_transform, &rect, request, shared_blurs)
        });
        // The layer paint's σ is local at the SAVE POINT, so the save-point
        // transform scales it to device — the same transform the recorder
        // used to pad the layer's bounds. (The list base alone would leave
        // a `scale(4); save_layer(blur σ5)` halo four times too narrow.)
        let effects = LayerEffects::of(
            layer.paint,
            effect_transform.max_scale(),
            effect_transform,
            true,
        );
        // z_denom = composite_slot − base_slot: the slot span this layer
        // hosts (children plus its composite), replay rebases slots to it.
        self.push_layer_frame(
            rect,
            (layer.composite_slot - layer.base_slot) as f32,
            layer.paint.clone(),
            layer.mask,
            layer.composite_z,
            effects,
        );
        if let Some(seed) = seed {
            self.emit_backdrop_seed(&rect, &seed);
        }
        Opened::Layer
    }

    /// `backdrop_seed` blurs what is already painted beneath `rect`: a
    /// same-key tile reuses the first tile's blur; otherwise the region
    /// (the keyed group's union, or just this layer) is padded by 3σ so
    /// edge taps read real scene, snapshotted from the parent target, and
    /// run through the blur chain.
    fn backdrop_seed(
        &mut self,
        base: &Matrix,
        effect_transform: &Matrix,
        rect: &Rect,
        request: BackdropRequest,
        shared_blurs: &mut FxHashMap<u64, SharedBlur>,
    ) -> BackdropSeed {
        let sigma = (request.sigma_local * effect_transform.max_scale()).max(0.05);
        if let Some(shared) = request
            .key
            .and_then(|key| shared_blurs.get(&key))
            .filter(|shared| (shared.sigma - sigma).abs() < 1e-3)
            .filter(|shared| shared.source == self.frame().src_texture)
        {
            self.stats.shared_backdrops += 1;
            return BackdropSeed {
                view: shared.view.clone(),
                region: shared.region,
                uv_max: shared.uv_max,
            };
        }
        let bounds = request
            .group_bounds
            .and_then(|union| base.map_rect(&union).intersect(&self.frame().cull_rect))
            .unwrap_or(*rect);
        let padded = bounds.expand((sigma * 3.0).ceil());
        let region = padded.intersect(&self.frame().cull_rect).unwrap_or(bounds);
        let blur = self.blur_of_target_region(&region, sigma);
        if let Some(key) = request.key {
            // First tile wins, as documented: a σ- or target-mismatched
            // tile blurs independently WITHOUT evicting the entry later
            // matching tiles reuse.
            shared_blurs.entry(key).or_insert(SharedBlur {
                view: blur.view.clone(),
                region,
                uv_max: blur.uv_max,
                sigma,
                source: self.frame().src_texture.clone(),
            });
        }
        self.stats.backdrops += 1;
        BackdropSeed {
            view: blur.view,
            region,
            uv_max: blur.uv_max,
        }
    }

    /// `emit_backdrop_seed` draws the blurred parent into the just-opened
    /// layer as its FIRST step — the glass every child paints over. z 0
    /// sits below every rebased child slot; the quad and the blur region
    /// are both absolute replay coords, so the layer's origin shift and the
    /// region uv agree.
    fn emit_backdrop_seed(&mut self, rect: &Rect, seed: &BackdropSeed) {
        let bind = self.emit.texture_bind(&seed.view);
        let frame = self.frames.last_mut().expect("layer frame just pushed");
        let mut record = self
            .emit
            .quad_record(frame, rect, [1.0, 1.0, 1.0, 1.0], 0.0);
        record.set_payload(PAYLOAD_GEOM, region_uv(&seed.region, seed.uv_max));
        self.emit.push_step(
            frame,
            PipelineKind::Draw(Frag::Image),
            BlendMode::SrcOver,
            record,
            Some(bind),
            None,
            0.0,
        );
    }

    /// `close_layer` emits the layer's last segment, pops the frame, and
    /// composites it into the parent. Restoring the outer slot base and
    /// group-alpha stack is replay's job — it saved them on the scope entry.
    pub(super) fn close_layer(&mut self) {
        self.emit_segment();
        let frame = self.frames.pop().expect("layer frame present");
        let info = frame.layer.expect("close_layer only on layer frames");
        self.composite_layer(&info);
    }

    /// `layer_rect` maps the recorded scope bounds into parent coords and
    /// intersects with what is visible.
    fn layer_rect(&mut self, base: &Matrix, scope_bounds: &Rect) -> Option<Rect> {
        if scope_bounds.is_empty() {
            return None;
        }
        base.map_rect(scope_bounds)
            .intersect(&self.frame().cull_rect)
    }

    /// `push_layer_frame` allocates a pooled offscreen and pushes it as the
    /// current target. `rect` is in absolute replay coords — it doubles as
    /// the children's cull rect and, minus its origin, the layer's pixel
    /// space. Group alpha from an enclosing elided scope lands ONCE, here,
    /// on the composite paint.
    pub(super) fn push_layer_frame(
        &mut self,
        rect: Rect,
        z_denom: f32,
        mut paint: Paint,
        mask: Option<MaskKind>,
        composite_z: f32,
        effects: LayerEffects,
    ) {
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

    /// `push_elision` pushes one elided scope's alpha, multiplied by
    /// whatever is already on the stack (Impeller's `distributed_opacity`).
    fn push_elision(&mut self, alpha: f32) {
        let combined = alpha * self.elisions.last().copied().unwrap_or(1.0);
        self.elisions.push(combined);
    }

    /// `plan_via_implicit_layer` renders one draw into its own layer and
    /// composites it with an advanced blend — the desugar for a
    /// destination-reading paint on a textured source. The inner draw uses
    /// explicit z (0.5 of the layer's 2.0 denominator), never slots, so the
    /// replay slot base stays untouched; the group-alpha stack is cleared
    /// for the inner draw because the composite paint absorbed it.
    pub(super) fn plan_via_implicit_layer(
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
        self.push_layer_frame(rect, 2.0, paint, None, z, LayerEffects::default());
        let outer_elisions = std::mem::take(&mut self.elisions);
        inner(self);
        self.elisions = outer_elisions;
        self.close_layer();
    }

    /// `plan_via_effect_layer_at` is the same path sized from DEVICE bounds.
    /// A glyph run takes it: glyph ink extents are not derivable at plan
    /// time, so the recorder carries the run's device bounds on the op and
    /// the layer sizes itself from those instead of from local geometry.
    pub(super) fn plan_via_effect_layer_at(
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
        self.push_layer_frame(rect, 2.0, composite, None, z, effects);
        let outer_elisions = std::mem::take(&mut self.elisions);
        inner(self);
        self.elisions = outer_elisions;
        self.close_layer();
    }

    /// `push_raster_frame` opens a list-raster cache texture as the current
    /// target. It is a plain frame, not a layer: closing it composites
    /// nothing, because the quad that samples the finished texture is an
    /// ordinary draw the caller emits in the parent afterwards.
    pub(super) fn push_raster_frame(&mut self, target: &FillTarget, z_denom: f32) {
        let attachments = self.pool.take_raster_attachments(target.size, self.format);
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
            z_denom,
            origin: Point::ZERO,
            transient: true,
            layer: None,
        });
    }

    /// `close_raster_frame` flushes the cache texture's last segment and
    /// pops it. Putting the suspended replay state back is the caller's job,
    /// the same way it is for a layer.
    pub(super) fn close_raster_frame(&mut self) {
        self.emit_segment();
        self.frames.pop().expect("raster frame present");
    }

    /// `composite_layer` draws the finished layer texture into the parent:
    /// a plain alpha/blend composite is one textured quad; an advanced
    /// blend snapshots the parent and blends in the fragment.
    fn composite_layer(&mut self, info: &LayerInfo) {
        if let Some(kind) = info.mask_composite {
            return self.composite_mask_layer(info, kind);
        }
        let (view, uv) = self.composite_source(info);
        let tint = alpha_tint(info.paint.color.a);
        if info.paint.blend_mode.is_pipeline_blendable() {
            let bind = self.emit.texture_bind(&view);
            let frame = self.frames.last_mut().expect("frame stack never empty");
            let mut record = self
                .emit
                .quad_record(frame, &info.rect, tint, info.composite_z);
            record.set_payload(PAYLOAD_GEOM, uv);
            self.emit.push_step(
                frame,
                PipelineKind::Draw(Frag::Image),
                info.paint.blend_mode,
                record,
                Some(bind),
                None,
                info.composite_z,
            );
        } else {
            let snapshot = self.break_pass(&info.rect);
            let bind = self.emit.blend_bind(&snapshot, &view);
            let frame = self.frames.last_mut().expect("frame stack never empty");
            let mut record = self
                .emit
                .quad_record(frame, &info.rect, tint, info.composite_z);
            record.set_payload(PAYLOAD_GEOM, uv);
            self.emit
                .set_blend_misc(frame, &mut record, info.paint.blend_mode);
            self.emit.push_step(
                frame,
                PipelineKind::Draw(Frag::BlendTexture),
                BlendMode::SrcOver,
                record,
                Some(bind),
                None,
                info.composite_z,
            );
        }
    }

    /// `composite_mask_layer` samples the mask texture across the WHOLE
    /// enclosing frame and multiplies it in via DstIn — outside the mask's
    /// rect the fragment forces coverage 0, which is what erases unmasked
    /// content.
    fn composite_mask_layer(&mut self, info: &LayerInfo, kind: MaskKind) {
        let size = layer_texture_size(&info.rect);
        let bind = self.emit.texture_bind(&info.resolve);
        let frame = self.frames.last_mut().expect("frame stack never empty");
        let extent = frame.cull_rect;
        let mut record = self.emit.quad_record(
            frame,
            &extent,
            alpha_tint(info.paint.color.a),
            info.composite_z,
        );
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
        self.emit.push_step(
            frame,
            PipelineKind::Draw(Frag::MaskComposite),
            BlendMode::DstIn,
            record,
            Some(bind),
            None,
            info.composite_z,
        );
    }

    /// `erase_frame_alpha` composites DstIn with zero source alpha over the
    /// whole frame — the "mask never rendered" result (coverage 0
    /// everywhere).
    fn erase_frame_alpha(&mut self, z: f32) {
        let frame = self.frames.last_mut().expect("frame stack never empty");
        let extent = frame.cull_rect;
        let record = self.emit.quad_record(frame, &extent, [0.0; 4], z);
        self.emit.push_step(
            frame,
            PipelineKind::Draw(Frag::Solid),
            BlendMode::DstIn,
            record,
            None,
            None,
            z,
        );
    }

    /// `frame` returns the innermost open target. Never empty: `new` pushes
    /// the main frame and every layer push has a matching pop.
    pub(super) fn frame(&self) -> &PassFrame {
        self.frames.last().expect("frame stack never empty")
    }

    pub(super) fn frame_mut(&mut self) -> &mut PassFrame {
        self.frames.last_mut().expect("frame stack never empty")
    }

    /// `elision_alpha` is the group alpha of the innermost elided opacity
    /// scope, or 1. Multiplies tints; never changes depth.
    pub(super) fn elision_alpha(&self) -> f32 {
        self.elisions.last().copied().unwrap_or(1.0)
    }
}

/// `layer_texture_size` is a layer's texture extent: its fractional rect,
/// ceil'd. The ONE formula — allocation, sampling, and the filter recipes
/// must agree or edge texels stretch.
pub(super) fn layer_texture_size(rect: &Rect) -> [u32; 2] {
    [
        rect.width.ceil().max(1.0) as u32,
        rect.height.ceil().max(1.0) as u32,
    ]
}
