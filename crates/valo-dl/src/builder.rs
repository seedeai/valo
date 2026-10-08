use std::sync::Arc;

use valo_geometry::{FillRule, Matrix, Path, PathBuilder, Rect};

use crate::list::Recording;
use crate::{Backdrop, Bounds, ClipOp, DisplayList, Image, MaskKind, Op, Paint, Sampling};

/// `DisplayListBuilder` records drawing commands into an immutable display list.
///
/// Recording is GPU-free and may run on any thread. The builder resolves bounds,
/// clips, layer extents, and ordering metadata so rendering does not need to
/// rediscover them.
pub struct DisplayListBuilder {
    ops: Vec<Op>,
    /// The open scopes, innermost last: the root scope at the bottom, closed
    /// by `build`, and one per open save or save layer.
    scopes: Vec<Scope>,
    /// The depth-slot counter: ONE line for the whole list (Impeller's
    /// `current_depth_`) — layer children continue it, never restart it.
    slots: u32,
    bounds: Bounds,
    /// The root's group-opacity oracle: what an enclosing layer that embeds
    /// this list learns about its children.
    root: GroupOpacity,
}

/// `Scope` is one open scope — the root, a save, or a save layer — and
/// everything its restore closes: the transform and clips its children see,
/// the clips made in it, and, for a save layer, what its restore backpatches.
struct Scope {
    transform: Matrix,
    clip: ClipState,
    /// Ops indexes of the clips made in this scope, awaiting the expiry its
    /// restore records.
    pending_clips: Vec<usize>,
    /// Set on a save layer's scope.
    layer: Option<LayerScope>,
}

/// `ClipState` is where a scope's children can show, list-root space:
/// unbounded where nothing clips them. Where they can show nowhere, the
/// scope is a nop: it records nothing more (Flutter's `is_nop`).
#[derive(Clone, Copy)]
struct ClipState {
    /// What a child is culled by: one that shows nowhere inside it is
    /// dropped, and its bounds are cropped by it.
    cull: Bounds,
    /// What the innermost open layer's content bounds are cropped by: the
    /// clips made since it opened, with its bounds hint. Clips around a
    /// layer do not crop its content, since a filter on the layer reads past
    /// them (Flutter's `layer_state`).
    layer_content: Bounds,
}

impl ClipState {
    /// `UNCLIPPED` is the root's: its children show anywhere.
    const UNCLIPPED: Self = Self {
        cull: Bounds::Unbounded,
        layer_content: Bounds::Unbounded,
    };

    /// `NOWHERE` is a nop scope's: nothing recorded in it could show.
    const NOWHERE: Self = Self {
        cull: Bounds::Empty,
        layer_content: Bounds::Empty,
    };

    /// `is_nop` reports whether the scope's children can show nowhere.
    fn is_nop(&self) -> bool {
        self.cull.is_empty() || self.layer_content.is_empty()
    }
}

/// `Footprint` is where one recorded child may show: cropped by the clip,
/// for culling and the list's bounds, and cropped only by the clips inside
/// the innermost layer, for that layer's content bounds. Neither is empty.
#[derive(Clone, Copy)]
struct Footprint {
    culled: Bounds,
    in_layer: Bounds,
}

impl Footprint {
    /// `cull_rect` is the rect a draw's op records for replay to cull it
    /// by: one that fills an unclipped list may show anywhere.
    fn cull_rect(&self) -> Rect {
        self.culled.rect().unwrap_or(Rect::EVERYTHING)
    }
}

/// Whether a group's alpha distributes over its children: every child
/// alpha-linear and no two overlapping, so scaling each child's source by α
/// equals compositing the group at α (Flutter's `is_group_opacity_compatible`).
/// Kept per open layer, and for the list's root so an embedded list answers
/// for its own children.
struct GroupOpacity {
    /// Alpha-linear + disjoint so far. Clips leave it alone.
    compatible: bool,
    /// The union of the children so far. A child inside it counts as an
    /// overlap even when it misses every earlier child: one rectangle to
    /// test instead of every child, at the price of a few group textures
    /// that were not needed (Flutter's `AccumulationRect`).
    union: Bounds,
}

impl GroupOpacity {
    fn new() -> Self {
        Self {
            compatible: true,
            union: Bounds::Empty,
        }
    }

    /// One more child: falsify on one that cannot take the group's opacity
    /// or on the first overlap (disjoint children are what makes shared-z
    /// elision legal).
    fn note(&mut self, bounds: Bounds, takes_group_opacity: bool) {
        if !self.compatible {
            return;
        }
        if !takes_group_opacity || self.union.intersects(&bounds) {
            self.compatible = false;
            return;
        }
        self.union = self.union.union(&bounds);
    }
}

/// Record-time state of an open `save_layer` scope.
struct LayerScope {
    /// The `Op::SaveLayer` to backpatch at restore.
    op_index: usize,
    /// Union of the children's content bounds (list-root space, cropped by
    /// the clips inside the layer and the hint).
    bounds: Bounds,
    /// Whether the children can take a group alpha.
    group: GroupOpacity,
    /// How far the composite paint's filters spread the layer's content,
    /// list-root units (3σ for a blur, as Flutter's display list pads it).
    reach: f32,
    /// The clip where the layer opens, cropped by the hint: the op's
    /// `clip_bounds`.
    opening_clip: Bounds,
    /// The op's `takes_group_opacity`: an enclosing group's alpha can ride
    /// the composite, and the composite is nothing but an alpha.
    takes_group_opacity: bool,
    /// The op's `floods_clip`.
    floods: bool,
    /// The layer opens on a backdrop, which needs a texture to seed.
    has_backdrop: bool,
    /// A caller-supplied bounds hint is a CROP; eliding a hinted layer
    /// would un-crop it. Conservative — Flutter tracks whether the bounds
    /// actually clipped (`kMayClipContents`); valo vetoes on any hint until
    /// a real caller needs the finer rule.
    hinted: bool,
}

impl Default for DisplayListBuilder {
    fn default() -> Self {
        Self::new()
    }
}

impl DisplayListBuilder {
    /// `new` creates an empty display-list builder.
    pub fn new() -> Self {
        Self {
            ops: Vec::new(),
            scopes: vec![Scope {
                transform: Matrix::IDENTITY,
                clip: ClipState::UNCLIPPED,
                pending_clips: Vec::new(),
                layer: None,
            }],
            slots: 0,
            bounds: Bounds::Empty,
            root: GroupOpacity::new(),
        }
    }

    // ── transform stack (canvas semantics) ─────────────────────────────────

    /// `save` preserves the current transform and clip until the matching `restore`.
    pub fn save(&mut self) {
        let top = self.top();
        let scope = Scope {
            transform: top.transform,
            clip: top.clip,
            pending_clips: Vec::new(),
            layer: None,
        };
        self.scopes.push(scope);
        self.ops.push(Op::Save);
    }

    /// `save_count` returns the current canvas save-stack depth.
    ///
    /// A new builder starts at one. Each `save` or save-layer operation
    /// increments the count, and each matched `restore` decrements it. Hosts
    /// can use the value to verify that callbacks leave shared canvas state balanced.
    pub fn save_count(&self) -> usize {
        self.scopes.len()
    }

    /// `save_layer` begins an offscreen layer composited with `paint` at `restore`.
    ///
    /// `bounds_hint` is a local-space crop, not merely an allocation hint;
    /// content outside it is discarded. Pass `None` to derive bounds from the
    /// recorded children and active clip.
    pub fn save_layer(&mut self, bounds_hint: Option<Rect>, paint: &Paint) {
        self.save_layer_inner(bounds_hint, paint, None, None);
    }

    /// `save_layer_mask` begins a mask layer closed by `restore`.
    ///
    /// The layer's pixels become luminance or alpha coverage according to
    /// `kind`, retaining enclosing content only where the mask has coverage.
    /// `bounds_hint` crops the mask in local space.
    pub fn save_layer_mask(&mut self, bounds_hint: Option<Rect>, kind: MaskKind) {
        let paint = Paint {
            blend_mode: crate::BlendMode::DstIn,
            ..Paint::default()
        };
        self.save_layer_inner(bounds_hint, &paint, Some(kind), None);
    }

    /// `save_layer_backdrop` begins a layer that OPENS pre-filled with the
    /// [`Backdrop`]-filtered scene beneath it (frosted glass). Children
    /// paint over that glass, and `restore` composites glass + children as
    /// one image with `paint` — so a group alpha fades them together
    /// (Flutter's `saveLayer(bounds, paint, backdrop)`).
    ///
    /// Without `bounds_hint` the layer covers the active clip — a backdrop
    /// reads everything beneath it, so a hint-less, clip-less list records
    /// unbounded bounds; hint the layer when the list will be embedded.
    pub fn save_layer_backdrop(
        &mut self,
        bounds_hint: Option<Rect>,
        paint: &Paint,
        backdrop: Backdrop,
    ) {
        let backdrop = (!backdrop.filter.is_nop()).then_some(backdrop);
        self.save_layer_inner(bounds_hint, paint, None, backdrop);
    }

    fn save_layer_inner(
        &mut self,
        bounds_hint: Option<Rect>,
        paint: &Paint,
        mask_composite: Option<MaskKind>,
        backdrop: Option<Backdrop>,
    ) {
        // A save layer's paint takes no mask blur: Flutter's display list
        // leaves it out of a save layer's attributes
        // (`kSaveLayerWithPaintFlags`), and Impeller has no way to apply one.
        let paint = Paint {
            mask_blur: None,
            ..paint.clone()
        };
        if self.top().clip.is_nop() || paint.is_invisible() {
            // Nothing the layer holds could show: a nop scope (Flutter's
            // `saveLayer` with no effect).
            self.save();
            self.top_mut().clip = ClipState::NOWHERE;
            return;
        }
        let top = self.top();
        let transform = top.transform;
        let root_hint =
            bounds_hint.map_or(Bounds::Unbounded, |hint| Bounds::of(hint).map(&transform));
        let reach = paint.device_effect_padding(&transform);
        let layer = LayerScope {
            op_index: self.ops.len(),
            bounds: Bounds::Empty,
            group: GroupOpacity::new(),
            reach,
            // The hint crops the children, so it joins every clip they see.
            opening_clip: top.clip.cull.intersect(&root_hint),
            takes_group_opacity: paint.takes_group_opacity(),
            floods: floods_its_clip(&paint, backdrop.is_some()),
            has_backdrop: backdrop.is_some(),
            hinted: bounds_hint.is_some(),
        };
        let clip = ClipState {
            // A filter on the layer reads past the clip, so the children are
            // culled by the clip widened by its reach (Flutter's
            // `resetDeviceCullRect` with the filter's input bounds).
            cull: top.clip.cull.expand(reach).intersect(&root_hint),
            layer_content: root_hint,
        };
        // Children keep counting on the SAME depth line (Impeller's global
        // numbering) — the layer's pass rebases against base_slot.
        self.ops.push(Op::SaveLayer {
            paint,
            mask_composite,
            scope_bounds: Bounds::Empty, // backpatched at restore
            clip_bounds: layer.opening_clip,
            base_slot: self.slots,
            composite_slot: 0,
            floods_clip: layer.floods,
            takes_group_opacity: layer.takes_group_opacity,
            can_elide: false,
            backdrop,
        });
        self.scopes.push(Scope {
            transform,
            clip,
            pending_clips: Vec::new(),
            layer: Some(layer),
        });
    }

    /// `restore` closes the most recent save, layer, or mask scope.
    ///
    /// An unmatched restore is ignored in release builds and triggers a debug assertion.
    pub fn restore(&mut self) {
        if self.scopes.len() == 1 {
            debug_assert!(false, "restore() without matching save()");
            return;
        }
        let scope = self.scopes.pop().expect("checked above");
        self.expire_clips(scope.pending_clips);
        if let Some(layer) = scope.layer {
            self.close_layer(layer);
        }
        self.ops.push(Op::Restore);
    }

    /// `translate` offsets subsequent drawing and clipping operations.
    pub fn translate(&mut self, tx: f32, ty: f32) {
        self.concat(&Matrix::translation(tx, ty));
    }

    /// `scale` scales subsequent drawing and clipping operations.
    pub fn scale(&mut self, sx: f32, sy: f32) {
        self.concat(&Matrix::scale(sx, sy));
    }

    /// `rotate` rotates subsequent drawing and clipping operations clockwise.
    ///
    /// Positive angles rotate clockwise in Valo's y-down coordinate system.
    pub fn rotate(&mut self, radians: f32) {
        self.concat(&Matrix::rotation(radians));
    }

    /// `concat` appends a transform for subsequent drawing and clipping operations.
    pub fn concat(&mut self, local: &Matrix) {
        let top = self.top_mut();
        top.transform = top.transform.then(local);
        self.ops.push(Op::Transform(*local));
    }

    // ── clips (depth slots; expiry backpatched when the scope closes) ──────

    /// `clip_rect` applies a rectangular clip until the current scope ends.
    pub fn clip_rect(&mut self, rect: impl Into<Rect>, op: ClipOp) {
        let rect = rect.into();
        self.clip_path(&rect_path(rect), FillRule::NonZero, op);
    }

    /// `clip_rrect` applies a rounded-rectangle clip with one corner radius.
    pub fn clip_rrect(&mut self, rect: impl Into<Rect>, radius: f32, op: ClipOp) {
        let rect = rect.into();
        self.clip_rrect_radii(rect, [radius; 4], op);
    }

    /// `clip_rrect_radii` applies a rounded-rectangle clip with per-corner radii.
    ///
    /// `radii` is ordered clockwise as `[top-left, top-right, bottom-right, bottom-left]`.
    pub fn clip_rrect_radii(&mut self, rect: impl Into<Rect>, radii: [f32; 4], op: ClipOp) {
        let rect = positive_rect(rect.into());
        let mut p = PathBuilder::new();
        p.rrect_radii(rect, radii);
        self.clip_path(&p.build(), FillRule::NonZero, op);
    }

    /// `clip_rrect_radii_elliptical` applies per-corner elliptical radii.
    ///
    /// Each clockwise corner is `[x_radius, y_radius]`, starting at the top-left.
    pub fn clip_rrect_radii_elliptical(
        &mut self,
        rect: impl Into<Rect>,
        radii: [[f32; 2]; 4],
        op: ClipOp,
    ) {
        let rect = positive_rect(rect.into());
        if let Some(circular) = circular_radii(radii) {
            return self.clip_rrect_radii(rect, circular, op);
        }
        let mut p = PathBuilder::new();
        p.rrect_radii_elliptical(rect, radii);
        self.clip_path(&p.build(), FillRule::NonZero, op);
    }

    /// `clip_path` applies a path clip until the current scope ends.
    ///
    /// Clips do NOT forfeit an enclosing layer's elision (Flutter's
    /// opacity distribution ignores clips too): a depth clip records its
    /// own expiry slot and works identically whether the group's children
    /// draw in a layer or in the parent, and child bounds are already
    /// clip-cropped when the disjointness check reads them. The Cupertino
    /// dialog depends on this — fade → clip → backdrop must keep the fade
    /// elidable or the glass snapshots a cleared offscreen.
    pub fn clip_path(&mut self, path: &Arc<Path>, fill_rule: FillRule, op: ClipOp) {
        if self.top().clip.is_nop() {
            return;
        }
        let bounds = Bounds::of(path.bounds()).map(&self.top().transform);
        self.shrink_clip(op, bounds);
        if self.top().clip.is_nop() {
            // The clip leaves the scope nothing, so the scope records
            // nothing more, this clip included (Flutter's `is_nop`).
            return;
        }
        let index = self.ops.len();
        self.top_mut().pending_clips.push(index);
        self.ops.push(Op::ClipPath {
            path: Arc::clone(path),
            fill_rule,
            op,
            expiry_slot: 0, // backpatched by expire_clips
        });
    }

    // ── draws (one slot each; bounds pre-clipped for the culling oracle) ───

    /// `draw_rect` records a filled or stroked rectangle.
    pub fn draw_rect(&mut self, rect: impl Into<Rect>, paint: &Paint) {
        let rect = rect.into();
        if matches!(paint.style, crate::PaintStyle::Stroke(_)) {
            // Stroked rects are stroked paths — one geometry pipeline.
            // Zero-area rects still stroke: Skia draws them as a line.
            return self.draw_path(&rect_path(rect), FillRule::NonZero, paint);
        }
        if rect.is_empty() {
            return;
        }
        if is_analytic_blur(paint) {
            self.record_rrect_blur(rect, [0.0; 4], paint);
            return;
        }
        self.record_draw(rect, paint, paint.takes_group_opacity(), |bounds, slot| {
            Op::DrawRect {
                rect,
                paint: paint.clone(),
                bounds,
                slot,
            }
        });
    }

    /// `draw_path` records a filled or stroked path.
    pub fn draw_path(&mut self, path: &Arc<Path>, fill_rule: FillRule, paint: &Paint) {
        if path.is_empty() {
            return;
        }
        let scale = self.top().transform.max_scale();
        let content_bounds = path.bounds().expand(paint.stroke_padding_at_scale(scale));
        self.record_draw(
            content_bounds,
            paint,
            paint.takes_group_opacity(),
            |bounds, slot| Op::DrawPath {
                path: Arc::clone(path),
                fill_rule,
                paint: paint.clone(),
                content_bounds,
                bounds,
                slot,
            },
        );
    }

    /// `draw_circle` records a filled or stroked circle.
    pub fn draw_circle(
        &mut self,
        center: impl Into<valo_geometry::Point>,
        radius: f32,
        paint: &Paint,
    ) {
        let mut p = PathBuilder::new();
        p.circle(center, radius);
        self.draw_path(&p.build(), FillRule::NonZero, paint);
    }

    /// `draw_rrect` records a rounded rectangle with one corner radius.
    pub fn draw_rrect(&mut self, rect: impl Into<Rect>, radius: f32, paint: &Paint) {
        let rect = rect.into();
        self.draw_rrect_radii(rect, [radius; 4], paint);
    }

    /// `draw_rrect_radii` records a rounded rectangle with per-corner radii.
    ///
    /// `radii` is ordered clockwise as `[top-left, top-right, bottom-right, bottom-left]`.
    pub fn draw_rrect_radii(&mut self, rect: impl Into<Rect>, radii: [f32; 4], paint: &Paint) {
        let rect = positive_rect(rect.into());
        if rect.is_empty() {
            return;
        }
        if is_analytic_blur(paint) {
            self.record_rrect_blur(rect, radii, paint);
            return;
        }
        let mut p = PathBuilder::new();
        p.rrect_radii(rect, radii);
        self.draw_path(&p.build(), FillRule::NonZero, paint);
    }

    /// `draw_rrect_radii_elliptical` records per-corner elliptical radii.
    ///
    /// Each clockwise corner is `[x_radius, y_radius]`, starting at the top-left.
    pub fn draw_rrect_radii_elliptical(
        &mut self,
        rect: impl Into<Rect>,
        radii: [[f32; 2]; 4],
        paint: &Paint,
    ) {
        let rect = positive_rect(rect.into());
        if let Some(circular) = circular_radii(radii) {
            return self.draw_rrect_radii(rect, circular, paint);
        }
        if rect.is_empty() {
            return;
        }
        let mut p = PathBuilder::new();
        p.rrect_radii_elliptical(rect, radii);
        self.draw_path(&p.build(), FillRule::NonZero, paint);
    }

    /// `draw_image` records the whole image into `dst`.
    ///
    /// It uses linear filtering and clamps at the image edges.
    pub fn draw_image(&mut self, image: &Image, dst: Rect, paint: &Paint) {
        let src = Rect::new(0.0, 0.0, image.width(), image.height());
        self.draw_image_rect(image, src, dst, Sampling::default(), paint);
    }

    /// `draw_image_rect` records a source region into `dst` with explicit sampling.
    ///
    /// `src` is measured in source pixels. Tiling applies when `src` extends
    /// beyond the image bounds.
    pub fn draw_image_rect(
        &mut self,
        image: &Image,
        src: Rect,
        dst: Rect,
        sampling: Sampling,
        paint: &Paint,
    ) {
        if dst.is_empty() || src.is_empty() {
            return;
        }
        self.record_draw(dst, paint, paint.takes_group_opacity(), |bounds, slot| {
            Op::DrawImage {
                image: image.clone(),
                src,
                dst,
                sampling,
                paint: paint.clone(),
                bounds,
                slot,
            }
        });
    }

    /// `draw_glyph_run` records positioned glyphs from one font and size.
    ///
    /// `local_bounds` must enclose the glyph ink in local coordinates. Valo
    /// retains the supplied font and glyph positions in the display list.
    pub fn draw_glyph_run(
        &mut self,
        font: std::sync::Arc<valo_text::Font>,
        size: f32,
        paint: &Paint,
        glyphs: Arc<Vec<crate::GlyphPos>>,
        local_bounds: Rect,
    ) {
        if glyphs.is_empty() {
            return;
        }
        let scale = self.top().transform.max_scale();
        let content_bounds = local_bounds.expand(paint.stroke_padding_at_scale(scale));
        // A run never takes a group's alpha (Flutter's rule): its glyphs may
        // overlap, nothing says whether they do, and an overlap faded glyph
        // by glyph darkens.
        let takes_group_opacity = false;
        self.record_draw(
            content_bounds,
            paint,
            takes_group_opacity,
            |bounds, slot| Op::GlyphRun {
                font,
                size,
                paint: paint.clone(),
                glyphs,
                content_bounds,
                bounds,
                slot,
            },
        );
    }

    /// `draw_display_list` records a nested display list by shared reference.
    pub fn draw_display_list(&mut self, list: &Arc<DisplayList>) {
        self.embed_display_list(list, false);
    }

    /// `draw_display_list_cached` records a nested list as a raster-cache candidate.
    ///
    /// Use it for stable, repeatedly drawn lists whose recording is expensive.
    /// The renderer may still replay the list directly when caching is unsuitable.
    pub fn draw_display_list_cached(&mut self, list: &Arc<DisplayList>) {
        self.embed_display_list(list, true);
    }

    fn embed_display_list(&mut self, list: &Arc<DisplayList>, cache: bool) {
        // The list answers for its own children (Flutter's
        // `can_apply_group_opacity`): a picture of one draw elides as the
        // draw would.
        let slots = list.depth_slots();
        self.record_op(
            list.bounds(),
            slots,
            list.supports_opacity(),
            |bounds, last_slot| Op::DrawDisplayList {
                list: Arc::clone(list),
                bounds,
                base_slot: last_slot - slots,
                cache,
            },
        );
    }

    // ── build ──────────────────────────────────────────────────────────────

    /// `build` consumes the builder and returns its immutable display list.
    ///
    /// Any unmatched save scopes are closed before the list is finalized.
    pub fn build(mut self) -> DisplayList {
        // Unbalanced saves are a recording bug, but a recoverable one: close
        // them so replay's stack discipline holds.
        while self.scopes.len() > 1 {
            self.restore();
        }
        let root = self.scopes.pop().expect("the root scope is never restored");
        self.expire_clips(root.pending_clips); // root-scope clips live to end-of-list
        DisplayList::new(
            self.ops,
            Recording {
                bounds: self.bounds,
                depth_slots: self.slots,
                supports_opacity: self.root.compatible,
            },
        )
    }

    // ── internals ──────────────────────────────────────────────────────────

    fn top(&self) -> &Scope {
        self.scopes.last().expect("scope stack never empty")
    }

    fn top_mut(&mut self) -> &mut Scope {
        self.scopes.last_mut().expect("scope stack never empty")
    }

    /// `close_layer` books a save layer's composite as a child of the scope
    /// around it and backpatches the layer's op, at its restore. Order
    /// matters: the layer's clips expired first (the caller did that), so
    /// their slots sit inside the children's span; the composite takes the
    /// NEXT slot on the same line.
    fn close_layer(&mut self, layer: LayerScope) {
        let footprint = self.root_footprint(layer.composite_region());
        let composite_slot = self.record_child(footprint, 1, layer.takes_group_opacity);
        self.backpatch_layer(&layer, composite_slot);
    }

    /// `backpatch_layer` writes what the layer's children decided into its
    /// op: its content, its composite's slot, and whether it may elide.
    fn backpatch_layer(&mut self, layer: &LayerScope, composite: u32) {
        let Op::SaveLayer {
            scope_bounds,
            composite_slot,
            can_elide,
            ..
        } = &mut self.ops[layer.op_index]
        else {
            unreachable!("LayerScope.op_index always points at SaveLayer");
        };
        *scope_bounds = layer.bounds;
        *composite_slot = composite;
        *can_elide = layer.can_elide();
    }

    /// One-quad closed-form blurred (r)rect; the quad spans the 3σ spread,
    /// which is the paint's effect padding (an analytic blur has no image
    /// filter).
    fn record_rrect_blur(&mut self, rect: Rect, radii: [f32; 4], paint: &Paint) {
        self.record_draw(rect, paint, paint.takes_group_opacity(), |bounds, slot| {
            Op::RRectBlur {
                rect,
                radii: valo_geometry::constrain_radii(&rect, radii),
                paint: paint.clone(),
                bounds,
                slot,
            }
        });
    }

    /// `record_draw` is the gate every draw goes through: a draw whose
    /// paint does nothing, or that shows nowhere, is dropped; any other is
    /// booked as a child of the scope, and `make` builds its op from the rect
    /// replay culls it by and its slot. `local` is the draw's ink in local
    /// coordinates, before the paint's effects; `takes_group_opacity` is
    /// whether a group's alpha can ride the draw: its paint's rule for a
    /// shape or an image, never for a glyph run.
    fn record_draw(
        &mut self,
        local: Rect,
        paint: &Paint,
        takes_group_opacity: bool,
        make: impl FnOnce(Rect, u32) -> Op,
    ) {
        if paint.is_nop() {
            return;
        }
        let local = paint.effect_bounds(local);
        self.record_op(local, 1, takes_group_opacity, make);
    }

    /// `record_op` records an op that shows within `local`: dropped when it
    /// shows nowhere, else booked as a child of `slots` slots and pushed as
    /// `make` builds it from its cull rect and its last slot.
    fn record_op(
        &mut self,
        local: Bounds,
        slots: u32,
        takes_group_opacity: bool,
        make: impl FnOnce(Rect, u32) -> Op,
    ) {
        let Some(footprint) = self.footprint(&local) else {
            return;
        };
        let last_slot = self.record_child(Some(footprint), slots, takes_group_opacity);
        self.ops.push(make(footprint.cull_rect(), last_slot));
    }

    /// `footprint` maps local bounds into list-root space and crops them by
    /// both clips; `None` = provably invisible, don't record.
    fn footprint(&self, local: &Bounds) -> Option<Footprint> {
        self.root_footprint(local.map(&self.top().transform))
    }

    /// `root_footprint` crops bounds already in list-root space.
    fn root_footprint(&self, bounds: Bounds) -> Option<Footprint> {
        let clip = self.top().clip;
        let footprint = Footprint {
            culled: bounds.intersect(&clip.cull),
            in_layer: bounds.intersect(&clip.layer_content),
        };
        let shows = !footprint.culled.is_empty() && !footprint.in_layer.is_empty();
        shows.then_some(footprint)
    }

    /// Intersect clips shrink the recorded clip bounds; Difference is kept
    /// conservative (bounds unchanged — correct, just not tighter).
    fn shrink_clip(&mut self, op: ClipOp, shape_bounds: Bounds) {
        if op == ClipOp::Difference {
            return;
        }
        let clip = &mut self.top_mut().clip;
        clip.cull = clip.cull.intersect(&shape_bounds);
        clip.layer_content = clip.layer_content.intersect(&shape_bounds);
    }

    /// `expire_clips` closes a scope's `pending` clips. Closing a scope that
    /// recorded clips consumes ONE slot — that slot is every pending clip's
    /// expiry: scope draws sit below it (ceilinged), later draws above it
    /// (free). This is how expiry stays record-time.
    fn expire_clips(&mut self, pending: Vec<usize>) {
        if pending.is_empty() {
            return;
        }
        self.slots += 1;
        for index in pending {
            let Op::ClipPath { expiry_slot, .. } = &mut self.ops[index] else {
                unreachable!("pending clips index only ClipPath ops");
            };
            *expiry_slot = self.slots;
        }
    }

    /// `record_child` books one child of the current scope: `slots` slots
    /// on the depth line and — when it shows anywhere — its `footprint` in
    /// the list's bounds and in the innermost open layer's oracle. Returns
    /// the child's last slot.
    fn record_child(
        &mut self,
        footprint: Option<Footprint>,
        slots: u32,
        takes_group_opacity: bool,
    ) -> u32 {
        self.slots += slots;
        if let Some(footprint) = footprint {
            self.bounds = self.bounds.union(&footprint.culled);
            self.note_layer_child(footprint.in_layer, takes_group_opacity);
        }
        self.slots
    }

    /// Feed the innermost open layer's oracle, or the root's outside any
    /// layer: union the layer's bounds and note the child for group opacity.
    fn note_layer_child(&mut self, bounds: Bounds, takes_group_opacity: bool) {
        let innermost = self
            .scopes
            .iter_mut()
            .rev()
            .find_map(|scope| scope.layer.as_mut());
        let Some(layer) = innermost else {
            self.root.note(bounds, takes_group_opacity);
            return;
        };
        layer.bounds = layer.bounds.union(&bounds);
        layer.group.note(bounds, takes_group_opacity);
    }
}

impl LayerScope {
    /// `can_elide` is the op's `can_elide`: the children take the group's
    /// alpha and none overlaps, the composite is nothing but that alpha, no
    /// backdrop needs a texture to seed and no hint crops the children.
    fn can_elide(&self) -> bool {
        self.group.compatible && self.takes_group_opacity && !self.has_backdrop && !self.hinted
    }

    /// `composite_region` is where the layer's composite shows, list-root
    /// space: its whole opening clip when it floods, else its content
    /// spread by the paint's filters.
    fn composite_region(&self) -> Bounds {
        if self.floods {
            self.opening_clip
        } else {
            self.bounds.expand(self.reach)
        }
    }
}

/// `floods_its_clip` reports whether a save layer with `paint` paints its
/// whole clip whatever its children draw: a destructive blend changes the
/// parent where the layer is transparent, a filter that colours
/// transparent pixels has output outside the children's ink, and a backdrop
/// layer OPENS full of the filtered parent.
fn floods_its_clip(paint: &Paint, has_backdrop: bool) -> bool {
    paint.blend_mode.is_destructive() || paint.reveals_transparent() || has_backdrop
}

/// `Some(circular)` when every corner's rx equals its ry — the case the
/// analytic rrect pipelines (blur shadows, uniform clips) can take.
fn circular_radii(radii: [[f32; 2]; 4]) -> Option<[f32; 4]> {
    radii
        .iter()
        .all(|[x, y]| x == y)
        .then(|| radii.map(|[x, _]| x))
}

// Flutter's RRect bridge accepts inverted edges and normalizes them before
// creating the engine round rect. CupertinoActivityIndicator relies on this.
fn positive_rect(rect: Rect) -> Rect {
    let x = if rect.width < 0.0 {
        rect.x + rect.width
    } else {
        rect.x
    };
    let y = if rect.height < 0.0 {
        rect.y + rect.height
    } else {
        rect.y
    };
    Rect::new(x, y, rect.width.abs(), rect.height.abs())
}

/// Solid + mask blur = the closed-form quad (Impeller's shadow gate,
/// Canvas::IsShadowBlurDrawOperation). Shaders/images take the filter path.
fn is_analytic_blur(paint: &Paint) -> bool {
    paint.mask_blur.is_some()
        && paint.shader.is_none()
        // The closed-form quad has nowhere to run a colour filter, so a
        // filtered shape takes the general layer path instead of silently
        // rendering its unfiltered colour.
        && paint.color_filter.is_none()
        && paint.effective_image_filter().is_none()
        && matches!(paint.style, crate::PaintStyle::Fill)
}

fn rect_path(r: Rect) -> Arc<Path> {
    let mut p = PathBuilder::new();
    p.rect(r);
    p.build()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{BlendMode, ImageFilter};
    use valo_geometry::Color;

    #[test]
    fn save_count_tracks_saves_layers_and_restores() {
        let mut builder = DisplayListBuilder::new();
        assert_eq!(builder.save_count(), 1);

        builder.save();
        assert_eq!(builder.save_count(), 2);

        builder.save_layer(None, &Paint::default());
        assert_eq!(builder.save_count(), 3);

        builder.restore();
        assert_eq!(builder.save_count(), 2);
        builder.restore();
        assert_eq!(builder.save_count(), 1);
    }

    #[test]
    fn rounded_rects_normalize_inverted_edges_like_flutter() {
        let mut builder = DisplayListBuilder::new();
        builder.draw_rrect(
            Rect::from_ltrb(-1.0, -10.0 / 3.0, 1.0, -10.0),
            1.0,
            &Paint::from_color(Color::WHITE),
        );

        let list = builder.build();
        let Op::DrawPath { path, .. } = &list.ops()[0] else {
            panic!("rounded rectangle should record as a path");
        };
        assert_eq!(
            path.bounds(),
            Rect::from_ltrb(-1.0, -10.0, 1.0, -10.0 / 3.0)
        );
    }
    fn red() -> Paint {
        Paint::from_color(Color::rgb(1.0, 0.0, 0.0))
    }

    fn alpha_layer(a: f32) -> Paint {
        Paint::from_color(Color::rgba(0.0, 0.0, 0.0, a))
    }

    fn find_clip(dl: &DisplayList) -> (&Op, u32) {
        for op in dl.ops() {
            if let Op::ClipPath { expiry_slot, .. } = op {
                return (op, *expiry_slot);
            }
        }
        panic!("no clip recorded");
    }

    /// Every recorded layer's `(scope_bounds, base_slot, composite_slot,
    /// can_elide)`, in recording order — so an enclosing layer comes before
    /// the layers nested inside it.
    fn layer_facts(dl: &DisplayList) -> Vec<(Bounds, u32, u32, bool)> {
        dl.ops()
            .iter()
            .filter_map(|op| match op {
                Op::SaveLayer {
                    scope_bounds,
                    base_slot,
                    composite_slot,
                    can_elide,
                    ..
                } => Some((*scope_bounds, *base_slot, *composite_slot, *can_elide)),
                _ => None,
            })
            .collect()
    }

    fn layer_clip_bounds(dl: &DisplayList) -> Bounds {
        dl.ops()
            .iter()
            .find_map(|op| match op {
                Op::SaveLayer { clip_bounds, .. } => Some(*clip_bounds),
                _ => None,
            })
            .expect("no layer recorded")
    }

    fn find_layer(dl: &DisplayList) -> (Bounds, u32, u32, bool) {
        *layer_facts(dl).first().expect("no layer recorded")
    }

    #[test]
    fn oracle_bounds_follow_transforms() {
        let mut b = DisplayListBuilder::new();
        b.save();
        b.translate(100.0, 50.0);
        b.draw_rect(Rect::new(0.0, 0.0, 10.0, 10.0), &red());
        b.restore();
        let dl = b.build();
        assert_eq!(
            dl.bounds(),
            Bounds::Bounded(Rect::new(100.0, 50.0, 10.0, 10.0))
        );
        assert_eq!(dl.draw_count(), 1);
        assert_eq!(dl.depth_slots(), 1);
    }

    #[test]
    fn clip_shrinks_recorded_draw_bounds() {
        let mut b = DisplayListBuilder::new();
        b.save();
        b.clip_rect(Rect::new(0.0, 0.0, 50.0, 50.0), ClipOp::Intersect);
        b.draw_rect(Rect::new(25.0, 25.0, 100.0, 100.0), &red());
        b.restore();
        let dl = b.build();
        assert_eq!(
            dl.bounds(),
            Bounds::Bounded(Rect::new(25.0, 25.0, 25.0, 25.0))
        );
    }

    #[test]
    fn fully_clipped_draw_is_dropped() {
        let mut b = DisplayListBuilder::new();
        b.save();
        b.clip_rect(Rect::new(0.0, 0.0, 10.0, 10.0), ClipOp::Intersect);
        b.draw_rect(Rect::new(500.0, 500.0, 10.0, 10.0), &red());
        b.restore();
        let dl = b.build();
        assert_eq!(dl.draw_count(), 0);
    }

    #[test]
    fn clip_expiry_is_the_restore_slot() {
        let mut b = DisplayListBuilder::new();
        b.draw_rect(Rect::new(0.0, 0.0, 10.0, 10.0), &red()); // slot 1
        b.save();
        b.clip_rect(Rect::new(0.0, 0.0, 50.0, 50.0), ClipOp::Intersect);
        b.draw_rect(Rect::new(0.0, 0.0, 10.0, 10.0), &red()); // slot 2
        b.restore(); // slot 3 = expiry
        b.draw_rect(Rect::new(0.0, 0.0, 10.0, 10.0), &red()); // slot 4
        let dl = b.build();
        let (_, expiry) = find_clip(&dl);
        assert_eq!(expiry, 3);
        assert_eq!(dl.depth_slots(), 4);
    }

    #[test]
    fn root_clip_expires_at_end_of_list() {
        let mut b = DisplayListBuilder::new();
        b.clip_rect(Rect::new(0.0, 0.0, 50.0, 50.0), ClipOp::Intersect);
        b.draw_rect(Rect::new(0.0, 0.0, 10.0, 10.0), &red()); // slot 1
        let dl = b.build();
        let (_, expiry) = find_clip(&dl);
        assert_eq!(expiry, 2, "root clips expire at the virtual end slot");
        assert_eq!(dl.depth_slots(), 2);
    }

    #[test]
    fn difference_clip_keeps_bounds_conservative() {
        let mut b = DisplayListBuilder::new();
        b.save();
        b.clip_rect(Rect::new(0.0, 0.0, 50.0, 50.0), ClipOp::Difference);
        b.draw_rect(Rect::new(0.0, 0.0, 100.0, 100.0), &red());
        b.restore();
        let dl = b.build();
        assert_eq!(
            dl.bounds(),
            Bounds::Bounded(Rect::new(0.0, 0.0, 100.0, 100.0))
        );
    }

    #[test]
    fn nested_list_folds_oracle_and_offsets_slots() {
        let mut inner = DisplayListBuilder::new();
        inner.draw_rect(Rect::new(0.0, 0.0, 10.0, 10.0), &red());
        inner.draw_rect(Rect::new(20.0, 0.0, 10.0, 10.0), &red());
        let inner = Arc::new(inner.build());

        let mut outer = DisplayListBuilder::new();
        outer.draw_rect(Rect::new(0.0, 0.0, 5.0, 5.0), &red()); // slot 1
        outer.translate(5.0, 5.0);
        outer.draw_display_list(&inner); // base_slot 1, child consumes 2
        outer.draw_rect(Rect::new(0.0, 0.0, 5.0, 5.0), &red()); // slot 4
        let outer = outer.build();

        assert_eq!(outer.draw_count(), 4);
        assert_eq!(outer.depth_slots(), 4);
        let base = outer
            .ops()
            .iter()
            .find_map(|op| match op {
                Op::DrawDisplayList { base_slot, .. } => Some(*base_slot),
                _ => None,
            })
            .unwrap();
        assert_eq!(base, 1);
    }

    #[test]
    fn nop_draws_are_dropped() {
        let mut b = DisplayListBuilder::new();
        b.draw_rect(Rect::new(0.0, 0.0, 0.0, 10.0), &red()); // empty rect
        b.draw_rect(
            Rect::new(0.0, 0.0, 10.0, 10.0),
            &Paint {
                color: Color::TRANSPARENT,
                blend_mode: BlendMode::SrcOver,
                ..Default::default()
            },
        );
        let dl = b.build();
        assert_eq!(dl.ops().len(), 0);
        assert_eq!(dl.bounds(), Bounds::Empty);
    }

    /// A list's bounds are empty when it draws nothing, a rectangle around
    /// what it draws, or unbounded when a draw fills a clip the list does
    /// not make.
    #[test]
    fn a_list_records_one_of_three_cases_of_bounds() {
        let fill_red = crate::ColorFilter::Blend(Color::rgb(1.0, 0.0, 0.0), BlendMode::Src);
        let flooding = Paint {
            image_filter: Some(ImageFilter::color(fill_red)),
            ..red()
        };
        assert_eq!(DisplayListBuilder::new().build().bounds(), Bounds::Empty);

        let mut b = DisplayListBuilder::new();
        b.draw_rect(Rect::new(4.0, 4.0, 4.0, 4.0), &red());
        assert_eq!(
            b.build().bounds(),
            Bounds::Bounded(Rect::new(4.0, 4.0, 4.0, 4.0))
        );

        let mut b = DisplayListBuilder::new();
        b.translate(1.0e9, 0.0);
        b.draw_rect(Rect::new(4.0, 4.0, 4.0, 4.0), &flooding);
        let unbounded = b.build();
        assert_eq!(unbounded.bounds(), Bounds::Unbounded);

        let mut b = DisplayListBuilder::new();
        b.scale(2.0, 2.0);
        b.draw_display_list(&Arc::new(unbounded));
        assert_eq!(
            b.build().bounds(),
            Bounds::Unbounded,
            "an embedded unbounded list stays unbounded wherever it is placed"
        );
    }

    /// A layer whose child fills a clip the layer does not crop has
    /// unbounded content; a backdrop layer with neither clip nor hint floods
    /// an unbounded clip.
    #[test]
    fn a_layer_records_unbounded_content_and_clip() {
        let fill_red = crate::ColorFilter::Blend(Color::rgb(1.0, 0.0, 0.0), BlendMode::Src);
        let mut b = DisplayListBuilder::new();
        b.clip_rect(Rect::new(0.0, 0.0, 32.0, 32.0), ClipOp::Intersect);
        b.save_layer(None, &alpha_layer(0.5));
        b.draw_rect(
            Rect::new(4.0, 4.0, 4.0, 4.0),
            &Paint {
                image_filter: Some(ImageFilter::color(fill_red)),
                ..red()
            },
        );
        b.restore();
        let dl = b.build();
        let (content, ..) = find_layer(&dl);
        assert_eq!(content, Bounds::Unbounded, "the clip is around the layer");
        assert_eq!(
            layer_clip_bounds(&dl),
            Bounds::Bounded(Rect::new(0.0, 0.0, 32.0, 32.0))
        );
        assert_eq!(
            dl.bounds(),
            Bounds::Bounded(Rect::new(0.0, 0.0, 32.0, 32.0))
        );

        let mut b = DisplayListBuilder::new();
        b.save_layer_backdrop(None, &Paint::default(), Backdrop::blur(4.0));
        b.restore();
        let dl = b.build();
        assert_eq!(layer_clip_bounds(&dl), Bounds::Unbounded);
        assert_eq!(dl.bounds(), Bounds::Unbounded);
    }

    /// A save layer whose composite is invisible records nothing inside: no
    /// draw, no slot (Flutter's nop `saveLayer`).
    #[test]
    fn an_invisible_layer_records_nothing_inside() {
        let mut b = DisplayListBuilder::new();
        b.save_layer(None, &alpha_layer(0.0));
        b.draw_rect(Rect::new(0.0, 0.0, 10.0, 10.0), &red());
        b.save_layer(None, &alpha_layer(0.5));
        b.draw_rect(Rect::new(20.0, 0.0, 10.0, 10.0), &red());
        b.restore();
        b.restore();
        b.draw_rect(Rect::new(40.0, 0.0, 10.0, 10.0), &red());
        let dl = b.build();
        assert!(
            matches!(
                dl.ops(),
                [
                    Op::Save,
                    Op::Save,
                    Op::Restore,
                    Op::Restore,
                    Op::DrawRect { slot: 1, .. }
                ]
            ),
            "{:?}",
            dl.ops()
        );
        assert_eq!(
            dl.bounds(),
            Bounds::Bounded(Rect::new(40.0, 0.0, 10.0, 10.0))
        );
        assert_eq!(dl.depth_slots(), 1);
    }

    /// A clip that leaves its scope nothing makes the scope a nop: neither
    /// the clip nor anything after it in the scope is recorded, and the
    /// scope's restore ends it.
    #[test]
    fn a_clip_that_leaves_nothing_makes_its_scope_a_nop() {
        let mut b = DisplayListBuilder::new();
        b.save();
        b.clip_rect(Rect::new(0.0, 0.0, 10.0, 10.0), ClipOp::Intersect);
        b.clip_rect(Rect::new(20.0, 0.0, 10.0, 10.0), ClipOp::Intersect);
        b.draw_rect(Rect::new(0.0, 0.0, 30.0, 30.0), &red());
        b.clip_rect(Rect::new(0.0, 0.0, 30.0, 30.0), ClipOp::Difference);
        b.save_layer(
            None,
            &Paint {
                blend_mode: BlendMode::Clear,
                ..Default::default()
            },
        );
        b.restore();
        b.restore();
        b.draw_rect(Rect::new(0.0, 0.0, 30.0, 30.0), &red());
        let dl = b.build();
        let clips = dl
            .ops()
            .iter()
            .filter(|op| matches!(op, Op::ClipPath { .. }))
            .count();
        assert_eq!(clips, 1, "only the clip that leaves something");
        assert!(!dl.ops().iter().any(|op| matches!(op, Op::SaveLayer { .. })));
        assert_eq!(dl.draw_count(), 1, "the draw after the scope");
        assert_eq!(
            dl.depth_slots(),
            2,
            "the first clip's expiry and the last draw"
        );
    }

    /// A layer whose hint misses its clip is still recorded, with an empty
    /// clip: a mask over nothing still erases what is beneath it.
    #[test]
    fn a_mask_whose_hint_misses_the_clip_is_recorded_with_an_empty_clip() {
        let mut b = DisplayListBuilder::new();
        b.clip_rect(Rect::new(0.0, 0.0, 10.0, 10.0), ClipOp::Intersect);
        b.save_layer_mask(Some(Rect::new(20.0, 0.0, 10.0, 10.0)), MaskKind::Alpha);
        b.draw_rect(Rect::new(20.0, 0.0, 10.0, 10.0), &red());
        b.restore();
        let dl = b.build();
        assert_eq!(layer_clip_bounds(&dl), Bounds::Empty);
        let (content, ..) = find_layer(&dl);
        assert_eq!(content, Bounds::Empty, "the hint culls every child");
    }

    // ── save layers (M4) ────────────────────────────────────────────────────

    #[test]
    fn layer_oracle_bounds_and_slots() {
        let mut b = DisplayListBuilder::new();
        b.draw_rect(Rect::new(0.0, 0.0, 10.0, 10.0), &red()); // slot 1
        b.save_layer(None, &alpha_layer(0.5)); // base_slot = 1
        b.draw_rect(Rect::new(20.0, 20.0, 30.0, 30.0), &red()); // slot 2
        b.draw_rect(Rect::new(60.0, 20.0, 30.0, 30.0), &red()); // slot 3
        b.restore(); // composite = slot 4, next on the same line
        b.draw_rect(Rect::new(0.0, 40.0, 10.0, 10.0), &red()); // slot 5
        let dl = b.build();

        let (bounds, base_slot, composite_slot, can_elide) = find_layer(&dl);
        assert_eq!(bounds, Bounds::Bounded(Rect::new(20.0, 20.0, 70.0, 30.0)));
        assert_eq!(base_slot, 1, "scope opened after one parent draw");
        assert_eq!(composite_slot, 4, "children keep the global line");
        assert!(
            can_elide,
            "disjoint SrcOver children + alpha-only composite"
        );
        assert_eq!(
            dl.depth_slots(),
            5,
            "one global depth line (Impeller's current_depth_)"
        );
        assert_eq!(dl.draw_count(), 5, "4 rects + the composite");
    }

    #[test]
    fn overlapping_children_forfeit_elision() {
        let mut b = DisplayListBuilder::new();
        b.save_layer(None, &alpha_layer(0.5));
        b.draw_rect(Rect::new(0.0, 0.0, 30.0, 30.0), &red());
        b.draw_rect(Rect::new(10.0, 10.0, 30.0, 30.0), &red()); // overlaps
        b.restore();
        let (_, _, _, can_elide) = find_layer(&b.build());
        assert!(!can_elide);
    }

    #[test]
    fn advanced_blend_composite_forfeits_elision() {
        let mut b = DisplayListBuilder::new();
        let paint = Paint {
            color: Color::rgba(0.0, 0.0, 0.0, 0.5),
            blend_mode: BlendMode::Multiply,
            ..Default::default()
        };
        b.save_layer(None, &paint);
        b.draw_rect(Rect::new(0.0, 0.0, 30.0, 30.0), &red());
        b.restore();
        let (_, _, _, can_elide) = find_layer(&b.build());
        assert!(!can_elide);
    }

    /// A colour-filtered layer's composite takes an alpha in before its
    /// filter, so an enclosing group's alpha cannot ride it (Flutter marks
    /// such a layer incompatible with group opacity).
    #[test]
    fn a_colour_filtered_layer_forfeits_its_groups_elision() {
        let mut b = DisplayListBuilder::new();
        b.save_layer(None, &alpha_layer(0.5));
        b.save_layer(
            None,
            &Paint {
                color_filter: Some(crate::ColorFilter::Blend(
                    Color::rgb(1.0, 0.0, 0.0),
                    BlendMode::SrcOver,
                )),
                ..Default::default()
            },
        );
        b.draw_rect(Rect::new(4.0, 4.0, 4.0, 4.0), &red());
        b.restore();
        b.restore();
        let layers = layer_facts(&b.build());
        assert!(!layers[0].3, "the group keeps its layer");
    }

    /// The recorded facts of the first layer in a list of one layer around
    /// one rect: whether it floods its clip and whether a group's alpha can
    /// ride it.
    fn layer_flood_and_opacity(paint: &Paint) -> (bool, bool) {
        let mut b = DisplayListBuilder::new();
        b.save_layer(None, paint);
        b.draw_rect(Rect::new(4.0, 4.0, 4.0, 4.0), &red());
        b.restore();
        b.build()
            .ops()
            .iter()
            .find_map(|op| match op {
                Op::SaveLayer {
                    floods_clip,
                    takes_group_opacity,
                    ..
                } => Some((*floods_clip, *takes_group_opacity)),
                _ => None,
            })
            .expect("a layer is recorded")
    }

    /// A layer floods its clip for a destructive blend or for a colour or
    /// image filter that colours transparent pixels; a group's alpha rides
    /// only a plain `SrcOver` composite.
    #[test]
    fn a_layer_records_its_flood_and_whether_a_group_alpha_can_ride_it() {
        let fill_red = crate::ColorFilter::Blend(Color::rgb(1.0, 0.0, 0.0), BlendMode::Src);
        let tint = crate::ColorFilter::Blend(Color::rgb(1.0, 0.0, 0.0), BlendMode::SrcIn);
        let layer = |paint: Paint| layer_flood_and_opacity(&paint);
        assert_eq!(layer(alpha_layer(0.5)), (false, true));
        assert_eq!(
            layer(Paint {
                blend_mode: BlendMode::SrcIn,
                ..Default::default()
            }),
            (true, false)
        );
        assert_eq!(
            layer(Paint {
                color_filter: Some(fill_red),
                ..Default::default()
            }),
            (true, false)
        );
        assert_eq!(
            layer(Paint {
                image_filter: Some(ImageFilter::color(fill_red)),
                ..Default::default()
            }),
            (true, false)
        );
        assert_eq!(
            layer(Paint {
                color_filter: Some(tint),
                ..Default::default()
            }),
            (false, false)
        );
    }

    /// A draw's colour filter stays inside its shape; its image filter, when
    /// it colours transparent pixels, fills the clip.
    #[test]
    fn a_draws_bounds_flood_for_its_image_filter_but_not_its_colour_filter() {
        let fill_red = crate::ColorFilter::Blend(Color::rgb(1.0, 0.0, 0.0), BlendMode::Src);
        let bounds = |paint: Paint| {
            let mut b = DisplayListBuilder::new();
            b.clip_rect(Rect::new(0.0, 0.0, 32.0, 32.0), ClipOp::Intersect);
            b.draw_rect(Rect::new(4.0, 4.0, 4.0, 4.0), &paint);
            b.build().bounds()
        };
        assert_eq!(
            bounds(Paint {
                color_filter: Some(fill_red),
                ..red()
            }),
            Bounds::Bounded(Rect::new(4.0, 4.0, 4.0, 4.0))
        );
        assert_eq!(
            bounds(Paint {
                image_filter: Some(ImageFilter::color(fill_red)),
                ..red()
            }),
            Bounds::Bounded(Rect::new(0.0, 0.0, 32.0, 32.0))
        );
    }

    #[test]
    fn destructive_layer_composite_floods_the_active_clip() {
        let mut b = DisplayListBuilder::new();
        b.clip_rect(Rect::new(4.0, 6.0, 80.0, 60.0), ClipOp::Intersect);
        b.save_layer(
            None,
            &Paint {
                blend_mode: BlendMode::SrcIn,
                ..Default::default()
            },
        );
        b.draw_rect(Rect::new(20.0, 20.0, 10.0, 10.0), &red());
        b.restore();
        let dl = b.build();
        let (bounds, ..) = find_layer(&dl);
        assert_eq!(
            bounds,
            Bounds::Bounded(Rect::new(20.0, 20.0, 10.0, 10.0)),
            "the content"
        );
        assert_eq!(
            layer_clip_bounds(&dl),
            Bounds::Bounded(Rect::new(4.0, 6.0, 80.0, 60.0))
        );
        assert_eq!(
            dl.bounds(),
            Bounds::Bounded(Rect::new(4.0, 6.0, 80.0, 60.0)),
            "the composite paints the clip"
        );
    }

    /// A clip around a blurred layer does not crop its content: the blur
    /// reads the content past the clip (Flutter's layer bounds).
    #[test]
    fn a_clip_around_a_layer_does_not_crop_its_content() {
        let mut b = DisplayListBuilder::new();
        b.clip_rect(Rect::new(0.0, 0.0, 50.0, 50.0), ClipOp::Intersect);
        b.save_layer(
            None,
            &Paint {
                image_filter: Some(ImageFilter::blur(4.0, 4.0)),
                ..Default::default()
            },
        );
        b.draw_rect(Rect::new(40.0, 10.0, 30.0, 10.0), &red());
        b.restore();
        let dl = b.build();
        let (bounds, ..) = find_layer(&dl);
        assert_eq!(bounds, Bounds::Bounded(Rect::new(40.0, 10.0, 30.0, 10.0)));
        assert_eq!(
            layer_clip_bounds(&dl),
            Bounds::Bounded(Rect::new(0.0, 0.0, 50.0, 50.0))
        );
    }

    /// A child just outside the clip still reaches inside it through a blur
    /// on its layer, so it is kept (Flutter culls a filtered layer's children
    /// by the clip widened by what the filter reads).
    #[test]
    fn a_blurred_layer_keeps_children_its_blur_reaches_across_the_clip() {
        let mut b = DisplayListBuilder::new();
        b.clip_rect(Rect::new(0.0, 0.0, 50.0, 50.0), ClipOp::Intersect);
        b.save_layer(
            None,
            &Paint {
                image_filter: Some(ImageFilter::blur(4.0, 4.0)),
                ..Default::default()
            },
        );
        b.draw_rect(Rect::new(55.0, 10.0, 10.0, 10.0), &red());
        b.draw_rect(Rect::new(80.0, 10.0, 10.0, 10.0), &red());
        b.restore();
        let (bounds, ..) = find_layer(&b.build());
        assert_eq!(
            bounds,
            Bounds::Bounded(Rect::new(55.0, 10.0, 10.0, 10.0)),
            "3σ reaches 12 past the clip: the first is kept, the second dropped"
        );
    }

    /// A save layer records no mask blur, so it neither blurs the layer nor
    /// widens the clip its children are culled by.
    #[test]
    fn a_save_layer_drops_its_mask_blur() {
        let mut b = DisplayListBuilder::new();
        b.clip_rect(Rect::new(0.0, 0.0, 50.0, 50.0), ClipOp::Intersect);
        b.save_layer(
            None,
            &Paint {
                mask_blur: Some(crate::MaskBlur::new(4.0)),
                ..alpha_layer(0.5)
            },
        );
        b.draw_rect(Rect::new(10.0, 10.0, 10.0, 10.0), &red());
        b.draw_rect(Rect::new(55.0, 10.0, 10.0, 10.0), &red());
        b.restore();
        let dl = b.build();
        let paint = dl.ops().iter().find_map(|op| match op {
            Op::SaveLayer { paint, .. } => Some(paint),
            _ => None,
        });
        assert_eq!(paint, Some(&alpha_layer(0.5)));
        let (bounds, ..) = find_layer(&dl);
        assert_eq!(bounds, Bounds::Bounded(Rect::new(10.0, 10.0, 10.0, 10.0)));
    }

    #[test]
    fn a_clip_inside_a_layer_crops_its_content() {
        let mut b = DisplayListBuilder::new();
        b.save_layer(None, &alpha_layer(0.5));
        b.clip_rect(Rect::new(0.0, 0.0, 30.0, 30.0), ClipOp::Intersect);
        b.draw_rect(Rect::new(20.0, 20.0, 30.0, 30.0), &red());
        b.restore();
        let dl = b.build();
        let (bounds, ..) = find_layer(&dl);
        assert_eq!(bounds, Bounds::Bounded(Rect::new(20.0, 20.0, 10.0, 10.0)));
        assert_eq!(layer_clip_bounds(&dl), Bounds::Unbounded);
    }

    #[test]
    fn a_stroked_path_records_its_ink_in_local_coordinates() {
        let mut b = DisplayListBuilder::new();
        b.translate(100.0, 0.0);
        b.draw_rect(
            Rect::new(10.0, 10.0, 20.0, 20.0),
            &Paint {
                style: crate::PaintStyle::Stroke(valo_geometry::Stroke {
                    join: valo_geometry::Join::Round,
                    ..valo_geometry::Stroke::new(4.0)
                }),
                ..red()
            },
        );
        let dl = b.build();
        let content_bounds = dl
            .ops()
            .iter()
            .find_map(|op| match op {
                Op::DrawPath { content_bounds, .. } => Some(*content_bounds),
                _ => None,
            })
            .expect("a stroked rect records a path");
        // Half the width times the round join's 1.5 on every side.
        assert_eq!(content_bounds, Rect::new(7.0, 7.0, 26.0, 26.0));
    }

    #[test]
    fn clip_inside_layer_keeps_elision() {
        let mut b = DisplayListBuilder::new();
        b.save_layer(None, &alpha_layer(0.5));
        b.clip_rect(Rect::new(0.0, 0.0, 50.0, 50.0), ClipOp::Intersect);
        b.draw_rect(Rect::new(0.0, 0.0, 30.0, 30.0), &red());
        b.restore();
        let (_, _, _, can_elide) = find_layer(&b.build());
        // A depth clip expires on its own slot either way; Flutter's
        // opacity distribution ignores clips too.
        assert!(can_elide);
    }

    #[test]
    fn an_embedded_list_of_disjoint_draws_keeps_elision() {
        let mut inner = DisplayListBuilder::new();
        inner.draw_rect(Rect::new(0.0, 0.0, 30.0, 30.0), &red());
        inner.draw_rect(Rect::new(40.0, 0.0, 30.0, 30.0), &red());
        let inner = Arc::new(inner.build());
        assert!(inner.supports_opacity());

        let mut b = DisplayListBuilder::new();
        b.save_layer(None, &alpha_layer(0.5));
        b.draw_display_list(&inner);
        b.restore();
        let (_, _, _, can_elide) = find_layer(&b.build());
        assert!(can_elide, "the list answers for its children");
    }

    #[test]
    fn an_embedded_list_of_overlapping_draws_forfeits_elision() {
        let mut inner = DisplayListBuilder::new();
        inner.draw_rect(Rect::new(0.0, 0.0, 30.0, 30.0), &red());
        inner.draw_rect(Rect::new(10.0, 10.0, 30.0, 30.0), &red());
        let inner = Arc::new(inner.build());
        assert!(!inner.supports_opacity());

        let mut b = DisplayListBuilder::new();
        b.save_layer(None, &alpha_layer(0.5));
        b.draw_display_list(&inner);
        b.restore();
        let (_, _, _, can_elide) = find_layer(&b.build());
        assert!(!can_elide);
    }

    #[test]
    fn two_embedded_lists_that_overlap_forfeit_elision() {
        let mut inner = DisplayListBuilder::new();
        inner.draw_rect(Rect::new(0.0, 0.0, 30.0, 30.0), &red());
        let inner = Arc::new(inner.build());

        let mut b = DisplayListBuilder::new();
        b.save_layer(None, &alpha_layer(0.5));
        b.draw_display_list(&inner);
        b.translate(10.0, 10.0);
        b.draw_display_list(&inner);
        b.restore();
        let (_, _, _, can_elide) = find_layer(&b.build());
        assert!(!can_elide);
    }

    #[test]
    fn a_child_inside_the_union_of_earlier_ones_forfeits_elision() {
        // Disjoint from both earlier rects, but inside their union: Flutter's
        // `AccumulationRect` calls that an overlap, and so does this.
        let mut b = DisplayListBuilder::new();
        b.save_layer(None, &alpha_layer(0.5));
        b.draw_rect(Rect::new(0.0, 0.0, 30.0, 30.0), &red());
        b.draw_rect(Rect::new(40.0, 40.0, 30.0, 30.0), &red());
        b.draw_rect(Rect::new(40.0, 0.0, 30.0, 30.0), &red());
        b.restore();
        let (_, _, _, can_elide) = find_layer(&b.build());
        assert!(!can_elide);
    }

    #[test]
    fn a_layer_composite_is_one_child_of_the_root() {
        let mut b = DisplayListBuilder::new();
        b.draw_rect(Rect::new(0.0, 0.0, 30.0, 30.0), &red());
        b.save_layer(None, &alpha_layer(0.5));
        b.draw_rect(Rect::new(10.0, 10.0, 30.0, 30.0), &red()); // overlaps the first
        b.restore();
        assert!(!b.build().supports_opacity());
    }

    #[test]
    fn bounds_hint_crops_the_scope() {
        let mut b = DisplayListBuilder::new();
        b.save_layer(Some(Rect::new(0.0, 0.0, 40.0, 40.0)), &alpha_layer(0.5));
        b.draw_rect(Rect::new(20.0, 20.0, 100.0, 100.0), &red());
        b.restore();
        let (bounds, ..) = find_layer(&b.build());
        assert_eq!(bounds, Bounds::Bounded(Rect::new(20.0, 20.0, 20.0, 20.0)));
    }

    #[test]
    fn clips_inside_layers_expire_within_the_scope_span() {
        let mut b = DisplayListBuilder::new();
        b.save_layer(None, &alpha_layer(0.5)); // base_slot = 0
        b.save();
        b.clip_rect(Rect::new(0.0, 0.0, 50.0, 50.0), ClipOp::Intersect);
        b.draw_rect(Rect::new(0.0, 0.0, 30.0, 30.0), &red()); // slot 1
        b.restore(); // slot 2 = expiry
        b.restore(); // composite = slot 3
        let dl = b.build();
        let (_, expiry) = find_clip(&dl);
        assert_eq!(expiry, 2, "expiry sits inside the layer's span");
        let (_, base_slot, composite_slot, _) = find_layer(&dl);
        assert_eq!((base_slot, composite_slot), (0, 3));
    }

    // ── mask + backdrop blur (M5) ───────────────────────────────────────────

    #[test]
    fn solid_mask_blur_records_the_analytic_op() {
        let mut b = DisplayListBuilder::new();
        let paint = Paint {
            mask_blur: Some(crate::MaskBlur::new(4.0)),
            ..red()
        };
        b.draw_rect(Rect::new(20.0, 20.0, 40.0, 40.0), &paint);
        b.draw_rrect(Rect::new(100.0, 20.0, 40.0, 40.0), 8.0, &paint);
        let dl = b.build();
        let blurs: Vec<_> = dl
            .ops()
            .iter()
            .filter_map(|op| match op {
                Op::RRectBlur { radii, bounds, .. } => Some((*radii, *bounds)),
                _ => None,
            })
            .collect();
        assert_eq!(blurs.len(), 2);
        assert_eq!(blurs[0].0, [0.0; 4]);
        assert_eq!(blurs[1].0, [8.0; 4]);
        // Bounds carry the ±3σ spread.
        assert_eq!(blurs[0].1, Rect::new(8.0, 8.0, 64.0, 64.0));
    }

    #[test]
    fn shader_mask_blur_stays_general_but_pads_bounds() {
        let mut b = DisplayListBuilder::new();
        let paint = Paint {
            mask_blur: Some(crate::MaskBlur::new(2.0)),
            shader: Some(crate::Shader::linear(
                valo_geometry::Point::new(0.0, 0.0),
                valo_geometry::Point::new(10.0, 0.0),
                Color::BLACK,
                Color::WHITE,
            )),
            color: Color::WHITE,
            ..Default::default()
        };
        b.draw_rect(Rect::new(10.0, 10.0, 20.0, 20.0), &paint);
        let dl = b.build();
        let Op::DrawRect { bounds, .. } = &dl.ops()[0] else {
            panic!("shader paints keep the general op");
        };
        assert_eq!(*bounds, Rect::new(4.0, 4.0, 32.0, 32.0));
    }

    #[test]
    fn hinted_layer_forfeits_elision() {
        let mut b = DisplayListBuilder::new();
        b.save_layer(Some(Rect::new(0.0, 0.0, 40.0, 40.0)), &alpha_layer(0.5));
        b.draw_rect(Rect::new(0.0, 0.0, 30.0, 30.0), &red());
        b.restore();
        let (_, _, _, can_elide) = find_layer(&b.build());
        assert!(!can_elide, "the hint is a crop; eliding would un-crop it");
    }

    // ── backdrop layers ─────────────────────────────────────────────────────

    /// A glass panel: a backdrop layer with nothing painted over it.
    fn glass(b: &mut DisplayListBuilder, rect: Rect, sigma: f32, key: Option<u64>) {
        b.save_layer_backdrop(
            Some(rect),
            &Paint::default(),
            Backdrop {
                filter: ImageFilter::blur(sigma, sigma),
                shared_key: key,
            },
        );
        b.restore();
    }

    #[test]
    fn shared_backdrops_group_by_key() {
        let mut b = DisplayListBuilder::new();
        glass(&mut b, Rect::new(0.0, 0.0, 50.0, 50.0), 8.0, Some(7));
        glass(&mut b, Rect::new(100.0, 0.0, 50.0, 50.0), 8.0, Some(7));
        glass(&mut b, Rect::new(0.0, 100.0, 50.0, 50.0), 8.0, None);
        let dl = b.build();
        let group = dl.backdrop_group(7).expect("key 7 recorded");
        // Each layer joins its group at close, contributing its scope bounds.
        assert_eq!(group.union_bounds, Rect::new(0.0, 0.0, 150.0, 50.0));
        assert_eq!(
            group.filter,
            Some(ImageFilter::blur(8.0, 8.0)),
            "one σ across the key: shareable"
        );
        assert_eq!(dl.draw_count(), 3, "each layer's composite is a draw");
        assert_eq!(dl.depth_slots(), 3);
    }

    #[test]
    fn mixed_sigma_under_one_key_clears_the_shared_sigma() {
        let mut b = DisplayListBuilder::new();
        glass(&mut b, Rect::new(0.0, 0.0, 50.0, 50.0), 4.0, Some(7));
        glass(&mut b, Rect::new(100.0, 0.0, 50.0, 50.0), 12.0, Some(7));
        let dl = b.build();
        let group = dl.backdrop_group(7).expect("key 7 recorded");
        assert_eq!(group.filter, None, "disagreeing σ cannot share one blur");
    }

    #[test]
    fn opacity_group_elides_over_a_backdrop_layer() {
        let mut b = DisplayListBuilder::new();
        b.save_layer(None, &alpha_layer(0.5));
        glass(&mut b, Rect::new(0.0, 0.0, 50.0, 50.0), 4.0, None);
        b.restore();
        let layers = layer_facts(&b.build());
        assert_eq!(layers.len(), 2, "the opacity group and the glass inside it");
        assert!(
            layers[0].3,
            "the group's alpha lands on the glass composite — the whole point \
             of backdrop-as-a-layer-property: glass keeps blurring while the \
             group fades"
        );
        assert!(!layers[1].3, "the glass itself needs a texture to seed");
    }

    /// The Cupertino dialog's exact recording shape: fade -> superellipse
    /// clip -> glass. The clip must NOT forfeit the fade's elision - a depth
    /// clip works identically whether the group's children draw in a layer
    /// or the parent, and eliding is what lets the glass snapshot the live
    /// scene instead of the fade's cleared offscreen.
    #[test]
    fn a_clip_does_not_forfeit_elision_around_glass() {
        let mut b = DisplayListBuilder::new();
        b.save_layer(None, &alpha_layer(0.5));
        b.save();
        let mut clip = PathBuilder::new();
        clip.rect(Rect::new(0.0, 0.0, 60.0, 60.0));
        b.clip_path(&clip.build(), FillRule::NonZero, ClipOp::Intersect);
        glass(&mut b, Rect::new(0.0, 0.0, 50.0, 50.0), 4.0, None);
        b.restore();
        b.restore();
        let layers = layer_facts(&b.build());
        assert_eq!(layers.len(), 2);
        assert!(layers[0].3, "the clipped fade still elides");
        assert!(!layers[1].3);
    }

    #[test]
    fn backdrop_reads_count_unshared_and_nested() {
        let mut child = DisplayListBuilder::new();
        glass(&mut child, Rect::new(0.0, 0.0, 50.0, 50.0), 4.0, None);
        let child = Arc::new(child.build());
        assert_eq!(child.backdrop_reads(), 1, "unshared reads count too");

        let mut parent = DisplayListBuilder::new();
        glass(&mut parent, Rect::new(0.0, 0.0, 50.0, 50.0), 8.0, Some(7));
        parent.draw_display_list(&child);
        let parent = parent.build();
        assert_eq!(parent.backdrop_reads(), 2, "own layer + the nested list's");

        let mut clean = DisplayListBuilder::new();
        clean.draw_rect(Rect::new(0.0, 0.0, 10.0, 10.0), &red());
        assert_eq!(clean.build().backdrop_reads(), 0);
    }

    #[cfg(feature = "serde")]
    #[test]
    fn serde_dump_is_readable_json() {
        // Dump-only by design (plan: diffs + bug reports, never persistence —
        // an Image can't be deserialized without a device).
        let mut b = DisplayListBuilder::new();
        b.translate(1.0, 2.0);
        b.draw_rect(Rect::new(0.0, 0.0, 10.0, 10.0), &red());
        let dl = b.build();
        let json: serde_json::Value = serde_json::to_value(&dl).unwrap();
        assert_eq!(json["ops"].as_array().unwrap().len(), dl.ops().len());
        assert!(json["ops"][1]["DrawRect"]["slot"].is_number());
    }
}
