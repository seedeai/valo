//! Walking one display list. This module is the only place that matches on
//! [`Op`]; everything it decides per draw is cull + depth + transform, then
//! it hands off to [`route`](super::route) or the layer lifecycle.
//!
//! [`ReplayState`] keeps the per-list walk state in ONE place — a scope
//! entry holds the transform together with what its `Restore` must undo,
//! because they push and pop at the same moments (Skia's `MCRec`,
//! Impeller's `CanvasStackEntry`). A materialized layer saves the outer
//! slot base and group-alpha stack on its scope entry rather than smuggling
//! them through the frame. A nested list gets a child state of its own, so
//! nothing about the parent's walk has to be saved and put back — including
//! the keyed backdrop-blur cache, so a blur computed for one drawing of a
//! retained list is never reused for a later drawing.

use std::sync::Arc;

use rustc_hash::FxHashMap;
use valo_dl::{DisplayList, Op};
use valo_geometry::{Matrix, Rect};

use crate::raster::{FillTarget, QuadSource, RasterVerdict};

use super::filters::SharedBlur;
use super::layers::{BackdropRequest, Opened, ResolvedLayer};
use super::route::{DrawSource, GlyphRun};
use super::Planner;

/// `ReplayState` is the walk state for one display list.
///
/// A nested list gets a child state so the parent walk is never saved and
/// restored — including the keyed backdrop-blur cache, which must not leak
/// from one drawing of a retained list into a later one. Each scope entry
/// carries the current transform together with what its `Restore` must undo
/// (Skia's `MCRec`, Impeller's `CanvasStackEntry`); a materialized layer
/// parks the outer slot base and group-alpha stack there rather than on the
/// frame.
pub(super) struct ReplayState {
    /// One entry per open scope; never empty while walking.
    scopes: Vec<ScopeEntry>,
    /// Maps recorded slots onto the current frame's depth line. Layer scopes
    /// rebase it to their own span and restore the outer value at close.
    slot_offset: i64,
    /// List root space → target coords, for the recorded (list-space) bounds.
    base: Matrix,
    /// Keyed backdrop blurs already computed for THIS list replay — later
    /// same-key layers seed from the first tile's blur (and see the scene as
    /// of that tile).
    shared_blurs: FxHashMap<u64, SharedBlur>,
}

struct ScopeEntry {
    /// Local → target. For layer children this stays PARENT coords — the
    /// layer's origin shift happens at MVP time in `emit`.
    transform: Matrix,
    on_restore: RestoreAction,
}

/// `RestoreAction` is what a scope's `Restore` must undo — decided when the
/// scope opened, carried on its entry because a `Restore` op itself says
/// nothing.
enum RestoreAction {
    /// Plain `save`: transform state only.
    None,
    /// An elided opacity layer: pop its group alpha.
    PopGroupAlpha,
    /// A materialized layer: put the outer replay state back, then close
    /// and composite the offscreen.
    CloseLayer {
        outer_slot_offset: i64,
        outer_elisions: Vec<f32>,
    },
}

impl ReplayState {
    /// `root` is the outermost list's state: identity base, slots from zero.
    pub fn root() -> Self {
        Self::nested(Matrix::IDENTITY, 0)
    }

    /// `nested` is a child list's state: `base` maps the child's root space
    /// into the current frame's coords and seeds its only open scope, and
    /// `slot_offset` places its recorded slots on the current depth line.
    pub fn nested(base: Matrix, slot_offset: i64) -> Self {
        Self {
            scopes: vec![ScopeEntry {
                transform: base,
                on_restore: RestoreAction::None,
            }],
            slot_offset,
            base,
            shared_blurs: FxHashMap::default(),
        }
    }

    fn top(&self) -> &ScopeEntry {
        self.scopes.last().expect("builder balances scopes")
    }

    fn push_scope(&mut self, on_restore: RestoreAction) {
        self.scopes.push(ScopeEntry {
            transform: self.top().transform,
            on_restore,
        });
    }
}

impl Planner<'_> {
    pub(super) fn replay_list(&mut self, dl: &DisplayList, state: &mut ReplayState) {
        let ops = dl.ops();
        let mut i = 0;
        while i < ops.len() {
            self.stats.ops += 1;
            match &ops[i] {
                Op::Save => state.push_scope(RestoreAction::None),
                Op::Transform(t) => {
                    let top = state.scopes.last_mut().expect("builder balances scopes");
                    top.transform = top.transform.then(t);
                }
                Op::SaveLayer {
                    paint,
                    mask_composite,
                    scope_bounds,
                    base_slot,
                    composite_slot,
                    can_elide,
                    backdrop_sigma,
                    backdrop_key,
                } => {
                    let backdrop = backdrop_sigma.map(|sigma_local| {
                        // A key whose tiles disagree on σ never shares (the
                        // recorder cleared the group's σ).
                        let key = backdrop_key
                            .filter(|&k| dl.backdrop_group(k).is_some_and(|g| g.sigma.is_some()));
                        BackdropRequest {
                            sigma_local,
                            key,
                            group_bounds: key
                                .and_then(|k| dl.backdrop_group(k))
                                .map(|group| group.union_bounds),
                        }
                    });
                    // The composite's z uses the OUTER slot base — it draws
                    // in the parent.
                    let composite_z = self.slot_z(state, *composite_slot);
                    let effect_transform = state.top().transform;
                    let list_base = state.base;
                    match self.open_layer(
                        &list_base,
                        &effect_transform,
                        ResolvedLayer {
                            paint,
                            mask: *mask_composite,
                            bounds: scope_bounds,
                            base_slot: *base_slot,
                            composite_slot: *composite_slot,
                            composite_z,
                            can_elide: *can_elide,
                            backdrop,
                        },
                        &mut state.shared_blurs,
                    ) {
                        Opened::Skip => {
                            i = skip_scope(ops, i) + 1;
                            continue;
                        }
                        Opened::Elided => state.push_scope(RestoreAction::PopGroupAlpha),
                        Opened::Layer => {
                            // Children rebase onto the layer's own slot span;
                            // group alpha was absorbed into the composite
                            // paint, so they start on an empty stack.
                            let outer_slot_offset =
                                std::mem::replace(&mut state.slot_offset, -(*base_slot as i64));
                            let outer_elisions = std::mem::take(&mut self.elisions);
                            state.push_scope(RestoreAction::CloseLayer {
                                outer_slot_offset,
                                outer_elisions,
                            });
                        }
                    }
                }
                Op::Restore => {
                    let entry = state.scopes.pop().expect("builder balances scopes");
                    match entry.on_restore {
                        RestoreAction::None => {}
                        RestoreAction::PopGroupAlpha => {
                            self.elisions.pop();
                        }
                        RestoreAction::CloseLayer {
                            outer_slot_offset,
                            outer_elisions,
                        } => {
                            state.slot_offset = outer_slot_offset;
                            self.elisions = outer_elisions;
                            self.close_layer();
                        }
                    }
                }
                Op::DrawRect {
                    rect,
                    paint,
                    bounds,
                    slot,
                } => {
                    if !self.culled(&state.base, bounds, 1) {
                        let z = self.slot_z(state, *slot);
                        let current = state.top().transform;
                        let device_bounds = state.base.map_rect(bounds);
                        self.plan_routed(DrawSource::Rect(rect), paint, &current, device_bounds, z);
                    }
                }
                Op::DrawPath {
                    path,
                    fill_rule,
                    paint,
                    bounds,
                    slot,
                } => {
                    if !self.culled(&state.base, bounds, 1) {
                        let z = self.slot_z(state, *slot);
                        let current = state.top().transform;
                        let source = DrawSource::Path {
                            path,
                            rule: *fill_rule,
                        };
                        let device_bounds = state.base.map_rect(bounds);
                        self.plan_routed(source, paint, &current, device_bounds, z);
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
                    if !self.culled(&state.base, bounds, 1) {
                        let z = self.slot_z(state, *slot);
                        let current = state.top().transform;
                        let source = DrawSource::Image {
                            image,
                            src,
                            dst,
                            sampling: *sampling,
                        };
                        let device_bounds = state.base.map_rect(bounds);
                        self.plan_routed(source, paint, &current, device_bounds, z);
                    }
                }
                Op::RRectBlur {
                    rect,
                    radii,
                    paint,
                    bounds,
                    slot,
                } => {
                    if !self.culled(&state.base, bounds, 1) {
                        let z = self.slot_z(state, *slot);
                        let current = state.top().transform;
                        let source = DrawSource::RRectBlur {
                            rect,
                            radii: *radii,
                        };
                        let device_bounds = state.base.map_rect(bounds);
                        self.plan_routed(source, paint, &current, device_bounds, z);
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
                    if !self.culled(&state.base, bounds, 1) {
                        let z = self.slot_z(state, *slot);
                        let current = state.top().transform;
                        let source = DrawSource::Glyphs(GlyphRun {
                            font,
                            size: *size,
                            glyphs,
                            device_bounds: state.base.map_rect(bounds),
                        });
                        let device_bounds = state.base.map_rect(bounds);
                        self.plan_routed(source, paint, &current, device_bounds, z);
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
                    let z = self.slot_z(state, *expiry_slot);
                    let current = state.top().transform;
                    self.plan_clip(path, *fill_rule, *op, &current, z);
                }
                Op::DrawDisplayList {
                    list,
                    bounds,
                    base_slot,
                    cache,
                } => {
                    if !self.culled(&state.base, bounds, list.draw_count()) {
                        if *cache && !self.filling_raster {
                            self.embed_cached_list(list, *base_slot, state);
                        } else {
                            self.replay_embedded(list, *base_slot, state);
                        }
                    }
                }
            }
            i += 1;
        }
    }

    /// `replay_embedded` walks a nested list inline: a CHILD [`ReplayState`]
    /// rooted at the embed transform, with the child's recorded slots placed
    /// after the parent's so the two lists share one depth line. The
    /// parent's own state is never touched, so nothing has to be put back.
    fn replay_embedded(&mut self, list: &Arc<DisplayList>, base_slot: u32, state: &ReplayState) {
        let embed = state.top().transform;
        let mut child = ReplayState::nested(embed, state.slot_offset + base_slot as i64);
        self.replay_list(list, &mut child);
    }

    /// `embed_cached_list` is a hinted embed: sample the cached raster as
    /// one quad, or fall back to inline replay — scheduling a fill when the
    /// cache asks for one. The fill renders as an extra pass and its quad
    /// samples the result in the same frame, so a fill never changes the
    /// pixels the user is looking at.
    fn embed_cached_list(&mut self, list: &Arc<DisplayList>, base_slot: u32, state: &ReplayState) {
        let embed = state.top().transform;
        // The composite quad is axis-aligned in pass coords, so rotated or
        // skewed embeds replay inline instead (Flutter skips integral
        // snapping under complex transforms for the same reason —
        // flutter#41654).
        let [_, shear_b, shear_c, ..] = embed.to_affine();
        if shear_b != 0.0 || shear_c != 0.0 || !embed.is_affine() {
            return self.replay_embedded(list, base_slot, state);
        }
        let verdict = self
            .emit
            .raster_verdict(self.rasters, list, embed.max_scale());
        match verdict {
            RasterVerdict::Quad(source) => self.plan_raster_quad(&source, &embed, base_slot, state),
            RasterVerdict::Fill(target) => {
                let source = target.quad_source();
                self.plan_one_raster_fill(list, target);
                self.plan_raster_quad(&source, &embed, base_slot, state);
            }
            RasterVerdict::Inline => self.replay_embedded(list, base_slot, state),
        }
    }

    /// `plan_raster_quad` draws one sampled quad standing in for a whole
    /// cached sub-list. At (near-)exact scale the origin snaps to integral
    /// device px and the destination takes the texture's own integer size,
    /// which is what makes it texel-perfect against inline replay (Flutter's
    /// `GetIntegralTransCTM` discipline).
    fn plan_raster_quad(
        &mut self,
        source: &QuadSource,
        embed: &Matrix,
        base_slot: u32,
        state: &ReplayState,
    ) {
        self.stats.raster_quads += 1;
        let mapped = embed.map_rect(&source.content_bounds);
        let ratio = embed.max_scale() / source.content_scale.max(1e-6);
        let exact = (ratio - 1.0).abs() < 1e-3;
        let extent = if exact {
            [source.size[0] as f32, source.size[1] as f32]
        } else {
            [source.size[0] as f32 * ratio, source.size[1] as f32 * ratio]
        };
        let dest = if exact {
            Rect::new(mapped.x.round(), mapped.y.round(), extent[0], extent[1])
        } else {
            mapped
        };
        let z = self.slot_z(state, base_slot);
        let frame = self.frames.last_mut().expect("frame stack never empty");
        self.emit
            .raster_quad_step(frame, &dest, extent, &source.view, z);
    }

    /// `plan_one_raster_fill` renders one cache entry mid-walk. Its pass
    /// emits BEFORE the current segment — exactly how every save layer
    /// renders before the composite that samples it — so the quad drawn
    /// right after this samples pixels already scheduled this frame.
    fn plan_one_raster_fill(&mut self, list: &Arc<DisplayList>, target: FillTarget) {
        self.stats.raster_fills += 1;
        self.filling_raster = true;
        // p_texture = scale · (p_list − origin): the translate applies first.
        let base = Matrix::scale(target.content_scale, target.content_scale).then(
            &Matrix::translation(-target.content_bounds.x, -target.content_bounds.y),
        );
        self.push_raster_frame(&target, (list.depth_slots() + 1) as f32);
        // The cached pixels are the list on its own: its slots start at zero
        // in the texture's depth space, and an enclosing elided group's
        // alpha belongs to the quad, not to what the texture holds.
        let outer_elisions = std::mem::take(&mut self.elisions);
        let mut state = ReplayState::nested(base, 0);
        self.replay_list(list, &mut state);
        self.close_raster_frame();
        self.elisions = outer_elisions;
        self.filling_raster = false;
    }

    /// `culled` rejects a draw whose recorded bounds miss the current frame.
    /// `bounds` are list-root space; `base` maps them into the frame's coords.
    fn culled(&mut self, base: &Matrix, bounds: &Rect, draws: u32) -> bool {
        let visible = base.map_rect(bounds).intersects(&self.frame().cull_rect);
        if !visible {
            self.stats.culled += draws;
        }
        !visible
    }

    /// `slot_z` is the depth value a slot occupies. The depth buffer clears
    /// to zero and draws test `GreaterEqual`; `slot_offset` rebases a layer's
    /// (or nested list's) slots onto the current line.
    fn slot_z(&self, state: &ReplayState, slot: u32) -> f32 {
        (state.slot_offset + slot as i64) as f32 / self.frame().z_denom
    }
}

/// `skip_scope` walks over a save/saveLayer scope's ops (used when a layer
/// is invisible) and returns the index of the matching `Restore`.
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
