use std::sync::Arc;

use valo_geometry::{FillRule, Matrix, Path, PathBuilder, Rect};

use crate::{ClipOp, DisplayList, Image, MaskKind, Op, Paint, Sampling};

/// `DisplayListBuilder` records drawing commands into an immutable display list.
///
/// Recording is GPU-free and may run on any thread. The builder resolves bounds,
/// clips, layer extents, and ordering metadata so rendering does not need to
/// rediscover them.
pub struct DisplayListBuilder {
    ops: Vec<Op>,
    scopes: Vec<Scope>,
    /// Open save layers (innermost last). Layer-scoped oracle state lives
    /// here; `Scope.is_layer` says which restore pops one.
    layers: Vec<LayerScope>,
    /// Shared backdrop keys seen so far: union of recorded tile bounds +
    /// tile count (the replay blurs each union once).
    backdrop_groups: Vec<crate::BackdropGroup>,
    /// Ops indexes of clips awaiting their expiry, one bucket per open scope
    /// (index 0 = the root scope, closed by `build`).
    pending_clips: Vec<Vec<usize>>,
    /// The depth-slot counter: ONE line for the whole list (Impeller's
    /// `current_depth_`) — layer children continue it, never restart it.
    slots: u32,
    bounds: Option<Rect>,
    draw_count: u32,
}

/// One save-scope's state: the transform and the device-space clip bounds
/// (`None` = unclipped). Both restore on `restore()`.
#[derive(Clone, Copy)]
struct Scope {
    transform: Matrix,
    clip: Option<Rect>,
    is_layer: bool,
}

/// Record-time state of an open `save_layer` scope.
struct LayerScope {
    /// The `Op::SaveLayer` to backpatch at restore.
    op_index: usize,
    /// Union of child draw bounds (list-root space, already clip∩hint-cropped).
    bounds: Option<Rect>,
    /// Children so far — for the pairwise-disjoint check (only consulted
    /// while `compatible` still holds).
    child_bounds: Vec<Rect>,
    /// Alpha-linear + disjoint so far (Flutter's
    /// can_distribute_opacity). Clips and nested lists falsify it.
    compatible: bool,
    /// ±3σ (device units) when the composite paint blurs.
    blur_pad: f32,
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
                clip: None,
                is_layer: false,
            }],
            layers: Vec::new(),
            backdrop_groups: Vec::new(),
            pending_clips: vec![Vec::new()],
            slots: 0,
            bounds: None,
            draw_count: 0,
        }
    }

    // ── transform stack (canvas semantics) ─────────────────────────────────

    /// `save` preserves the current transform and clip until the matching `restore`.
    pub fn save(&mut self) {
        self.scopes.push(Scope {
            is_layer: false,
            ..*self.top()
        });
        self.pending_clips.push(Vec::new());
        self.ops.push(Op::Save);
    }

    /// `save_layer` begins an offscreen layer composited with `paint` at `restore`.
    ///
    /// `bounds_hint` is a local-space crop, not merely an allocation hint;
    /// content outside it is discarded. Pass `None` to derive bounds from the
    /// recorded children and active clip.
    pub fn save_layer(&mut self, bounds_hint: Option<Rect>, paint: &Paint) {
        self.save_layer_inner(bounds_hint, paint, None);
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
        self.save_layer_inner(bounds_hint, &paint, Some(kind));
    }

    fn save_layer_inner(
        &mut self,
        bounds_hint: Option<Rect>,
        paint: &Paint,
        mask_composite: Option<MaskKind>,
    ) {
        let device_hint = bounds_hint.map(|h| self.top().transform.map_rect(&h));
        let mut scope = Scope {
            is_layer: true,
            ..*self.top()
        };
        // The hint crops children: fold it into the scope clip so child
        // bounds (and everything derived) come pre-cropped.
        if let Some(h) = device_hint {
            scope.clip = Some(match scope.clip {
                None => h,
                Some(c) => c.intersect(&h).unwrap_or_default(),
            });
        }
        // A filter that changes transparent black has output outside child
        // ink. Its input coverage is therefore the explicit/active clip, or
        // the renderer's eventual surface limit when no clip is known yet.
        let floods_scope = paint.blend_mode.is_destructive()
            || paint
                .color_filter
                .is_some_and(|filter| filter.modifies_transparent_black())
            || paint
                .image_filter
                .as_ref()
                .is_some_and(|filter| filter.modifies_transparent_black());
        let flooded_bounds = floods_scope.then(|| scope.clip.unwrap_or(Rect::EVERYTHING));
        self.scopes.push(scope);
        self.pending_clips.push(Vec::new());
        self.layers.push(LayerScope {
            op_index: self.ops.len(),
            bounds: flooded_bounds,
            child_bounds: Vec::new(),
            compatible: true,
            // Blurred layers spread ink past their children:
            // pad the recorded bounds so the texture holds the falloff.
            blur_pad: paint.device_effect_padding(&self.top().transform),
        });
        // Children keep counting on the SAME depth line (Impeller's global
        // numbering) — the layer's pass rebases against base_slot.
        self.ops.push(Op::SaveLayer {
            paint: paint.clone(),
            mask_composite,
            scope_bounds: Rect::default(), // backpatched at restore
            base_slot: self.slots,
            composite_slot: 0,
            can_elide: false,
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
        self.expire_scope_clips(); // uses the CURRENT (possibly layer) counter
        if scope.is_layer {
            self.close_layer();
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
        let rect = rect.into();
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
        let rect = rect.into();
        if let Some(circular) = circular_radii(radii) {
            return self.clip_rrect_radii(rect, circular, op);
        }
        let mut p = PathBuilder::new();
        p.rrect_radii_elliptical(rect, radii);
        self.clip_path(&p.build(), FillRule::NonZero, op);
    }

    /// `clip_path` applies a path clip until the current scope ends.
    pub fn clip_path(&mut self, path: &Arc<Path>, fill_rule: FillRule, op: ClipOp) {
        let bounds = self.top().transform.map_rect(&path.bounds());
        self.shrink_clip(op, bounds);
        // Clips forfeit elision: correct in principle now that elided
        // children keep their own slots, but kept conservative until a
        // scene needs it (stricter than Flutter/Impeller, on purpose).
        if let Some(layer) = self.layers.last_mut() {
            layer.compatible = false;
        }
        self.pending_clips
            .last_mut()
            .expect("root scope")
            .push(self.ops.len());
        self.ops.push(Op::ClipPath {
            path: Arc::clone(path),
            fill_rule,
            op,
            bounds,
            expiry_slot: 0, // backpatched by expire_scope_clips
        });
    }

    // ── draws (one slot each; bounds pre-clipped for the culling oracle) ───

    /// `draw_rect` records a filled or stroked rectangle.
    pub fn draw_rect(&mut self, rect: impl Into<Rect>, paint: &Paint) {
        let rect = rect.into();
        if paint.is_nop() {
            return;
        }
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
        let Some(bounds) = self.clipped_device_bounds(&paint.effect_bounds(rect)) else {
            return; // fully clipped at record time
        };
        let slot = self.take_draw_slot(bounds, supports_opacity(paint));
        self.ops.push(Op::DrawRect {
            rect,
            paint: paint.clone(),
            bounds,
            slot,
        });
    }

    /// `draw_path` records a filled or stroked path.
    pub fn draw_path(&mut self, path: &Arc<Path>, fill_rule: FillRule, paint: &Paint) {
        if path.is_empty() || paint.is_nop() {
            return;
        }
        let scale = self.top().transform.max_scale();
        let local = paint.effect_bounds(path.bounds().expand(paint.stroke_padding_at_scale(scale)));
        let Some(bounds) = self.clipped_device_bounds(&local) else {
            return;
        };
        let slot = self.take_draw_slot(bounds, supports_opacity(paint));
        self.ops.push(Op::DrawPath {
            path: Arc::clone(path),
            fill_rule,
            paint: paint.clone(),
            bounds,
            slot,
        });
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
        let rect = rect.into();
        if rect.is_empty() || paint.is_nop() {
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
        let rect = rect.into();
        if let Some(circular) = circular_radii(radii) {
            return self.draw_rrect_radii(rect, circular, paint);
        }
        if rect.is_empty() || paint.is_nop() {
            return;
        }
        let mut p = PathBuilder::new();
        p.rrect_radii_elliptical(rect, radii);
        self.draw_path(&p.build(), FillRule::NonZero, paint);
    }

    /// `backdrop_blur` blurs existing target pixels beneath `rect`.
    ///
    /// `sigma` is measured in local units. The active clip shapes the result;
    /// later draws appear above it.
    pub fn backdrop_blur(&mut self, rect: Rect, sigma: f32) {
        self.record_backdrop(rect, sigma, None);
    }

    /// `backdrop_blur_shared` shares one blur across regions with the same key.
    ///
    /// Sharing reduces filter work but snapshots the background when the first
    /// keyed region is reached. Use one key only for regions over the same
    /// background.
    pub fn backdrop_blur_shared(&mut self, rect: Rect, sigma: f32, key: u64) {
        self.record_backdrop(rect, sigma, Some(key));
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
        if dst.is_empty() || src.is_empty() || paint.is_nop() {
            return;
        }
        let Some(bounds) = self.clipped_device_bounds(&paint.effect_bounds(dst)) else {
            return;
        };
        let slot = self.take_draw_slot(bounds, supports_opacity(paint));
        self.ops.push(Op::DrawImage {
            image: image.clone(),
            src,
            dst,
            sampling,
            paint: paint.clone(),
            bounds,
            slot,
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
        if glyphs.is_empty() || paint.is_nop() {
            return;
        }
        let scale = self.top().transform.max_scale();
        let padded = paint.effect_bounds(local_bounds.expand(paint.stroke_padding_at_scale(scale)));
        let Some(bounds) = self.clipped_device_bounds(&padded) else {
            return;
        };
        // Shader text desugars into a two-draw layer at plan time; group
        // opacity can't ride its children (it would apply twice).
        let distributes = supports_opacity(paint) && paint.shader.is_none();
        let slot = self.take_draw_slot(bounds, distributes);
        self.ops.push(Op::GlyphRun {
            font,
            size,
            paint: paint.clone(),
            glyphs,
            bounds,
            slot,
        });
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
        let Some(child_bounds) = list.bounds() else {
            return; // draws nothing
        };
        let Some(bounds) = self.clipped_device_bounds(&child_bounds) else {
            return;
        };
        let base_slot = self.slots;
        self.slots += list.depth_slots();
        self.draw_count += list.draw_count();
        self.union_bounds(bounds);
        // Conservative: a nested list's internal structure is opaque here.
        self.note_layer_child(bounds, false);
        self.ops.push(Op::DrawDisplayList {
            list: Arc::clone(list),
            bounds,
            base_slot,
            cache,
        });
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
        self.expire_scope_clips(); // root-scope clips live to end-of-list
        DisplayList::new(
            self.ops,
            self.bounds,
            self.draw_count,
            self.slots,
            self.backdrop_groups,
        )
    }

    // ── internals ──────────────────────────────────────────────────────────

    fn top(&self) -> &Scope {
        self.scopes.last().expect("scope stack never empty")
    }

    fn top_mut(&mut self) -> &mut Scope {
        self.scopes.last_mut().expect("scope stack never empty")
    }

    /// Backpatch the layer's oracle at its restore. Order matters: the
    /// layer's clips expired first (caller did that), so their slots sit
    /// inside the children's span; the composite takes the NEXT slot on the
    /// same line.
    fn close_layer(&mut self) {
        let layer = self.layers.pop().expect("is_layer scope had a LayerScope");
        self.slots += 1; // the composite's slot, next after the children's span
        let mut scope_bounds = layer.bounds.unwrap_or_default();
        if layer.blur_pad > 0.0 && !scope_bounds.is_empty() {
            scope_bounds = scope_bounds.expand(layer.blur_pad);
        }

        let Op::SaveLayer {
            paint,
            mask_composite: _,
            scope_bounds: sb,
            base_slot: _,
            composite_slot,
            can_elide,
        } = &mut self.ops[layer.op_index]
        else {
            unreachable!("LayerScope.op_index always points at SaveLayer");
        };
        *sb = scope_bounds;
        *composite_slot = self.slots;
        *can_elide = layer.compatible && paint.is_opacity_only();

        let supports = paint.blend_mode == crate::BlendMode::SrcOver;
        self.draw_count += 1; // the composite draws
        self.union_bounds(scope_bounds);
        self.note_layer_child(scope_bounds, supports);
    }

    /// One-quad closed-form blurred (r)rect; the quad spans the 3σ spread.
    fn record_rrect_blur(&mut self, rect: Rect, radii: [f32; 4], paint: &Paint) {
        let Some(bounds) = self.clipped_device_bounds(&rect.expand(paint.mask_padding())) else {
            return;
        };
        let slot = self.take_draw_slot(bounds, supports_opacity(paint));
        self.ops.push(Op::RRectBlur {
            rect,
            radii: valo_geometry::constrain_radii(&rect, radii),
            paint: paint.clone(),
            bounds,
            slot,
        });
    }

    fn record_backdrop(&mut self, rect: Rect, sigma: f32, shared_key: Option<u64>) {
        if rect.is_empty() || sigma <= 0.0 {
            return;
        }
        let Some(bounds) = self.clipped_device_bounds(&rect) else {
            return;
        };
        // Reads the target, so it can never share an elided layer's z.
        let slot = self.take_draw_slot(bounds, false);
        if let Some(key) = shared_key {
            self.note_backdrop_group(key, bounds, sigma);
        }
        self.ops.push(Op::BackdropBlur {
            rect,
            sigma,
            shared_key,
            bounds,
            slot,
        });
    }

    fn note_backdrop_group(&mut self, key: u64, bounds: Rect, sigma: f32) {
        match self.backdrop_groups.iter_mut().find(|g| g.key == key) {
            Some(group) => {
                group.union_bounds = group.union_bounds.union(&bounds);
                group.tiles += 1;
                if group.sigma != Some(sigma) {
                    group.sigma = None; // mixed σ under one key: no sharing
                }
            }
            None => self.backdrop_groups.push(crate::BackdropGroup {
                key,
                union_bounds: bounds,
                tiles: 1,
                sigma: Some(sigma),
            }),
        }
    }

    /// Draw bounds in list-root space, pre-intersected with the clip stack;
    /// `None` = provably invisible, don't record.
    fn clipped_device_bounds(&self, local: &Rect) -> Option<Rect> {
        let device = self.top().transform.map_rect(local);
        match self.top().clip {
            None => Some(device),
            Some(clip) => device.intersect(&clip),
        }
    }

    /// Intersect clips shrink the recorded clip bounds; Difference is kept
    /// conservative (bounds unchanged — correct, just not tighter).
    fn shrink_clip(&mut self, op: ClipOp, shape_bounds: Rect) {
        if op == ClipOp::Difference {
            return;
        }
        let top = self.top_mut();
        top.clip = Some(match top.clip {
            None => shape_bounds,
            Some(c) => c.intersect(&shape_bounds).unwrap_or_default(), // empty = all clipped
        });
    }

    /// Closing a scope that recorded clips consumes ONE slot — that slot is
    /// every pending clip's expiry: scope draws sit below it (ceilinged),
    /// later draws above it (free). This is how expiry stays record-time.
    fn expire_scope_clips(&mut self) {
        let pending = self.pending_clips.pop().expect("scope stack never empty");
        if !pending.is_empty() {
            self.slots += 1;
            for idx in pending {
                let Op::ClipPath { expiry_slot, .. } = &mut self.ops[idx] else {
                    unreachable!("pending_clips indexes only ClipPath ops");
                };
                *expiry_slot = self.slots;
            }
        }
        if self.pending_clips.is_empty() {
            self.pending_clips.push(Vec::new()); // keep the root bucket alive
        }
    }

    fn take_draw_slot(&mut self, device_bounds: Rect, supports_opacity: bool) -> u32 {
        self.slots += 1;
        self.draw_count += 1;
        self.union_bounds(device_bounds);
        self.note_layer_child(device_bounds, supports_opacity);
        self.slots
    }

    fn union_bounds(&mut self, b: Rect) {
        self.bounds = Some(match self.bounds {
            Some(cur) => cur.union(&b),
            None => b,
        });
    }

    /// Feed the innermost open layer's oracle: union its bounds; falsify
    /// compatibility on an alpha-nonlinear child or the first overlap
    /// (pairwise-disjoint is what makes shared-z elision legal).
    fn note_layer_child(&mut self, bounds: Rect, supports_opacity: bool) {
        let Some(layer) = self.layers.last_mut() else {
            return;
        };
        layer.bounds = Some(match layer.bounds {
            Some(cur) => cur.union(&bounds),
            None => bounds,
        });
        if !layer.compatible {
            return;
        }
        if !supports_opacity {
            layer.compatible = false;
            return;
        }
        if layer
            .child_bounds
            .iter()
            .any(|prior| prior.intersects(&bounds))
        {
            layer.compatible = false;
            return;
        }
        layer.child_bounds.push(bounds);
    }
}

/// Group opacity distributes over a child iff scaling its src by α equals
/// compositing the group at α: true for SrcOver and Plus (both linear in
/// src), false for dst-multiplying and advanced modes.
fn supports_opacity(paint: &Paint) -> bool {
    // A colour filter is affine, not linear: distributing the group's alpha
    // into the paint colour would filter the DIMMED colour, and
    // `matrix(c · α) != matrix(c) · α` wherever the matrix translates or
    // clamps. Filtered draws keep their own layer.
    paint.color_filter.is_none()
        && paint.effective_image_filter().is_none()
        && matches!(
            paint.blend_mode,
            crate::BlendMode::SrcOver | crate::BlendMode::Plus
        )
}

/// Solid + mask blur = the closed-form quad (Impeller's shadow gate,
/// Canvas::IsShadowBlurDrawOperation). Shaders/images take the filter path.
/// `Some(circular)` when every corner's rx equals its ry — the case the
/// analytic rrect pipelines (blur shadows, uniform clips) can take.
fn circular_radii(radii: [[f32; 2]; 4]) -> Option<[f32; 4]> {
    radii
        .iter()
        .all(|[x, y]| x == y)
        .then(|| radii.map(|[x, _]| x))
}

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
    use crate::BlendMode;
    use valo_geometry::Color;

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

    fn find_layer(dl: &DisplayList) -> (Rect, u32, u32, bool) {
        for op in dl.ops() {
            if let Op::SaveLayer {
                scope_bounds,
                base_slot,
                composite_slot,
                can_elide,
                ..
            } = op
            {
                return (*scope_bounds, *base_slot, *composite_slot, *can_elide);
            }
        }
        panic!("no layer recorded");
    }

    #[test]
    fn oracle_bounds_follow_transforms() {
        let mut b = DisplayListBuilder::new();
        b.save();
        b.translate(100.0, 50.0);
        b.draw_rect(Rect::new(0.0, 0.0, 10.0, 10.0), &red());
        b.restore();
        let dl = b.build();
        assert_eq!(dl.bounds(), Some(Rect::new(100.0, 50.0, 10.0, 10.0)));
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
        assert_eq!(dl.bounds(), Some(Rect::new(25.0, 25.0, 25.0, 25.0)));
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
        assert_eq!(dl.bounds(), Some(Rect::new(0.0, 0.0, 100.0, 100.0)));
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
        assert_eq!(dl.bounds(), None);
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
        assert_eq!(bounds, Rect::new(20.0, 20.0, 70.0, 30.0));
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
        let (bounds, ..) = find_layer(&b.build());
        assert_eq!(bounds, Rect::new(4.0, 6.0, 80.0, 60.0));
    }

    #[test]
    fn clip_inside_layer_forfeits_elision() {
        let mut b = DisplayListBuilder::new();
        b.save_layer(None, &alpha_layer(0.5));
        b.clip_rect(Rect::new(0.0, 0.0, 50.0, 50.0), ClipOp::Intersect);
        b.draw_rect(Rect::new(0.0, 0.0, 30.0, 30.0), &red());
        b.restore();
        let (_, _, _, can_elide) = find_layer(&b.build());
        assert!(!can_elide);
    }

    #[test]
    fn bounds_hint_crops_the_scope() {
        let mut b = DisplayListBuilder::new();
        b.save_layer(Some(Rect::new(0.0, 0.0, 40.0, 40.0)), &alpha_layer(0.5));
        b.draw_rect(Rect::new(20.0, 20.0, 100.0, 100.0), &red());
        b.restore();
        let (bounds, ..) = find_layer(&b.build());
        assert_eq!(bounds, Rect::new(20.0, 20.0, 20.0, 20.0));
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
    fn shared_backdrops_group_by_key() {
        let mut b = DisplayListBuilder::new();
        b.backdrop_blur_shared(Rect::new(0.0, 0.0, 50.0, 50.0), 8.0, 7);
        b.backdrop_blur_shared(Rect::new(100.0, 0.0, 50.0, 50.0), 8.0, 7);
        b.backdrop_blur(Rect::new(0.0, 100.0, 50.0, 50.0), 8.0);
        let dl = b.build();
        let group = dl.backdrop_group(7).expect("key 7 recorded");
        assert_eq!(group.tiles, 2);
        assert_eq!(group.union_bounds, Rect::new(0.0, 0.0, 150.0, 50.0));
        assert_eq!(dl.draw_count(), 3, "each tile is a draw");
        assert_eq!(dl.depth_slots(), 3);
    }

    #[test]
    fn backdrop_inside_layer_forfeits_elision() {
        let mut b = DisplayListBuilder::new();
        b.save_layer(None, &alpha_layer(0.5));
        b.backdrop_blur(Rect::new(0.0, 0.0, 50.0, 50.0), 4.0);
        b.restore();
        let (_, _, _, can_elide) = find_layer(&b.build());
        assert!(!can_elide, "a dst-reading tile can't share the composite z");
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
