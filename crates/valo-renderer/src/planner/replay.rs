//! Walking one display list. This module is the only place that matches on
//! [`Op`]: the walk is a dispatch, each op handed to the scope stack, the
//! layer lifecycle, a clip, an embed, or [`Planner::replay_draw`], which
//! culls a draw and routes it.
//!
//! [`ReplayState`] keeps the per-list walk state in ONE place — a scope
//! entry holds the transform, the depth base and the group alpha together
//! with what its `Restore` must undo, because they push and pop at the same
//! moments (Skia's `MCRec`, Impeller's `CanvasStackEntry` with its
//! `distributed_opacity`). A materialized layer's composite rides its scope
//! entry until the restore closes it (Impeller's `SaveLayerState`). A nested
//! list gets a child state of its own, so nothing about the parent's walk
//! has to be saved and put back — including the keyed backdrop-blur cache,
//! so a blur computed for one drawing of a retained list is never reused for
//! a later drawing.

use std::sync::Arc;

use rustc_hash::FxHashMap;
use valo_dl::{DisplayList, MaskBlur, Op, Paint};
use valo_geometry::{Matrix, Rect};

use super::backdrop::SharedBackdrop;
use super::draw_state::DrawState;
use super::layers::{LayerComposite, Opened, ResolvedLayer};
use super::source::{DrawSource, GlyphRun, Shape};
use super::Planner;

/// `ReplayState` is the walk state for one display list.
pub(super) struct ReplayState {
    /// One entry per open scope; never empty while walking.
    scopes: Vec<ScopeEntry>,
    /// List root space → target coords, for the recorded (list-space) bounds.
    base: Matrix,
    /// Keyed backdrop blurs already computed for THIS list replay — later
    /// same-key layers seed from the first tile's blur (and see the scene as
    /// of that tile).
    shared_backdrops: FxHashMap<u64, SharedBackdrop>,
    /// Set while replaying INTO a raster-cache texture, and in every list
    /// nested inside it: a cacheable embed met during a fill replays inline.
    filling_raster: bool,
}

/// `ScopeEntry` is one open scope of the walk.
struct ScopeEntry {
    /// Local → target. For layer children this stays PARENT coords — the
    /// layer's origin shift happens at MVP time in `emit`.
    transform: Matrix,
    /// Places the list's recorded slots on the line of the outermost list
    /// being replayed into the target: a nested list's after its
    /// embedder's. A layer's own depth range rebases its children.
    slot_offset: u32,
    /// The alpha of the elided opacity layers this scope is inside, which
    /// every draw in it is drawn at (Impeller's `distributed_opacity`). A
    /// materialized layer's children start again at 1: its composite takes
    /// the value of the scope around it.
    group_alpha: f32,
    on_restore: RestoreAction,
}

/// `RestoreAction` is what a scope's `Restore` must do — decided when the
/// scope opened, carried on its entry because a `Restore` op itself says
/// nothing.
pub(super) enum RestoreAction {
    /// A plain `save` or an elided layer: popping the entry is all of it.
    None,
    /// A materialized layer: close its target and draw it into the parent.
    CloseLayer(LayerComposite),
}

impl ReplayState {
    /// `root` is the outermost list's state: identity base, slots from zero,
    /// no group alpha.
    pub fn root() -> Self {
        Self::rooted_at(Matrix::IDENTITY, 0, 1.0, false)
    }

    /// `embedded` is the state of a list embedded in the current scope with
    /// its slots from `base_slot`: rooted at the scope's transform, its
    /// slots placed on the same depth line, drawn at the scope's group
    /// alpha.
    pub fn embedded(&self, base_slot: u32) -> Self {
        let top = self.top();
        Self::rooted_at(
            top.transform,
            top.slot_offset + base_slot,
            top.group_alpha,
            self.filling_raster,
        )
    }

    /// `raster_fill` is the state of a list replayed into its cache texture
    /// under `base`: slots from zero, no group alpha.
    pub fn raster_fill(base: Matrix) -> Self {
        Self::rooted_at(base, 0, 1.0, true)
    }

    fn rooted_at(base: Matrix, slot_offset: u32, group_alpha: f32, filling_raster: bool) -> Self {
        Self {
            scopes: vec![ScopeEntry {
                transform: base,
                slot_offset,
                group_alpha,
                on_restore: RestoreAction::None,
            }],
            base,
            shared_backdrops: FxHashMap::default(),
            filling_raster,
        }
    }

    fn top(&self) -> &ScopeEntry {
        self.scopes.last().expect("builder balances scopes")
    }

    /// `base` maps the list's root space into the target's coordinates.
    pub fn base(&self) -> &Matrix {
        &self.base
    }

    /// `device_bounds` maps recorded bounds, list-root space, into the
    /// target's coordinates.
    pub fn device_bounds(&self, bounds: &Rect) -> Rect {
        self.base.map_rect(bounds)
    }

    /// `absolute` places a slot the list recorded on the target's depth
    /// line.
    pub fn absolute(&self, slot: u32) -> u32 {
        self.top().slot_offset + slot
    }

    /// `transform` appends `local` to the current scope's transform.
    fn transform(&mut self, local: &Matrix) {
        let top = self.scopes.last_mut().expect("builder balances scopes");
        top.transform = top.transform.then(local);
    }

    /// `save` opens a plain scope.
    fn save(&mut self) {
        let top = self.top();
        self.push_scope(top.slot_offset, top.group_alpha, RestoreAction::None);
    }

    /// `save_elided` opens an elided opacity layer's scope: its children are
    /// drawn in the parent at the group alpha times `alpha` (Impeller's
    /// opacity peephole).
    fn save_elided(&mut self, alpha: f32) {
        let top = self.top();
        self.push_scope(
            top.slot_offset,
            top.group_alpha * alpha,
            RestoreAction::None,
        );
    }

    /// `save_layer` opens a materialized layer's scope: its children are
    /// drawn into the layer at no group alpha, and its restore draws
    /// `composite`.
    fn save_layer(&mut self, composite: LayerComposite) {
        let top = self.top();
        let on_restore = RestoreAction::CloseLayer(composite);
        self.push_scope(top.slot_offset, 1.0, on_restore);
    }

    /// `restore` closes the innermost scope and returns what it leaves to do.
    fn restore(&mut self) -> RestoreAction {
        let entry = self.scopes.pop().expect("builder balances scopes");
        entry.on_restore
    }

    fn push_scope(&mut self, slot_offset: u32, group_alpha: f32, on_restore: RestoreAction) {
        self.scopes.push(ScopeEntry {
            transform: self.top().transform,
            slot_offset,
            group_alpha,
            on_restore,
        });
    }
}

impl Planner<'_> {
    /// `replay_list` walks `list`'s ops in `state`.
    pub(super) fn replay_list(&mut self, list: &DisplayList, state: &mut ReplayState) {
        let mut ops = OpCursor::new(list.ops());
        while let Some(op) = ops.next() {
            self.plan.stats.ops += 1;
            match op {
                Op::Save => state.save(),
                Op::Transform(local) => state.transform(local),
                Op::SaveLayer { composite_slot, .. } => {
                    // The composite draws in the parent: the parent's depth
                    // line, the save point's transform, the scope's group
                    // alpha.
                    let at = self.draw_state(state, *composite_slot);
                    let layer = ResolvedLayer::of(op, list, state, at);
                    match self.open_layer(layer, &mut state.shared_backdrops) {
                        Opened::Skip => ops.skip_scope(),
                        Opened::Elided(alpha) => state.save_elided(alpha),
                        Opened::Layer(composite) => state.save_layer(composite),
                    }
                }
                Op::Restore => {
                    if let RestoreAction::CloseLayer(composite) = state.restore() {
                        self.close_layer(composite);
                    }
                }
                Op::ClipPath {
                    path,
                    fill_rule,
                    op: clip_op,
                    expiry_slot,
                } => {
                    // Never bounds-culled: an Intersect ceiling covers the
                    // whole target MINUS the shape.
                    let at = self.draw_state(state, *expiry_slot);
                    self.drawing().clip(path, *fill_rule, *clip_op, &at);
                }
                Op::DrawDisplayList {
                    list,
                    bounds,
                    base_slot,
                    cache,
                } => self.replay_embed(list, bounds, *base_slot, *cache, state),
                Op::DrawRect { .. }
                | Op::DrawPath { .. }
                | Op::DrawImage { .. }
                | Op::RRectBlur { .. }
                | Op::GlyphRun { .. } => self.replay_draw(RecordedDraw::of(op, state), state),
            }
        }
    }

    /// `replay_draw` culls one recorded draw against the target and routes
    /// what is left.
    fn replay_draw(&mut self, draw: RecordedDraw<'_>, state: &ReplayState) {
        if self.contexts.top().area.culls(&draw.bounds) {
            self.plan.stats.culled += 1;
            return;
        }
        let at = self.at_slot(state, draw.slot);
        self.plan_routed(draw.source, draw.paint, &at);
    }

    /// `replay_embed` draws an embedded list: culled whole when its bounds
    /// miss the target, else from its cached raster when the host hinted it
    /// (and this walk is not filling one), else inline.
    fn replay_embed(
        &mut self,
        list: &Arc<DisplayList>,
        bounds: &Rect,
        base_slot: u32,
        cache: bool,
        state: &ReplayState,
    ) {
        if self.contexts.top().area.culls(&state.device_bounds(bounds)) {
            self.plan.stats.culled += list.draw_count();
        } else if cache && !state.filling_raster {
            self.embed_cached_list(list, base_slot, state);
        } else {
            self.replay_embedded(list, base_slot, state);
        }
    }

    /// `replay_embedded` walks a nested list inline in a CHILD
    /// [`ReplayState`], so the parent's own state is never touched and
    /// nothing has to be put back.
    pub(super) fn replay_embedded(
        &mut self,
        list: &Arc<DisplayList>,
        base_slot: u32,
        state: &ReplayState,
    ) {
        self.replay_list(list, &mut state.embedded(base_slot));
    }

    /// `draw_state` is what a draw the list recorded at `slot` is handed:
    /// the scope's transform and group alpha, and the slot's depth. The
    /// depth buffer clears to zero and draws test `GreaterEqual`.
    pub(super) fn draw_state(&self, state: &ReplayState, slot: u32) -> DrawState {
        self.at_slot(state, state.absolute(slot))
    }

    /// `at_slot` is [`Planner::draw_state`] for a slot already on the
    /// target's depth line.
    fn at_slot(&self, state: &ReplayState, slot: u32) -> DrawState {
        let top = state.top();
        DrawState {
            transform: top.transform,
            z: self.contexts.top().depth.z(slot),
            alpha: top.group_alpha,
        }
    }
}

/// `RecordedDraw` is one recorded draw pinned to one replay: its source and
/// paint, its recorded bounds in the target's coordinates, and its slot on
/// the target's depth line.
struct RecordedDraw<'a> {
    source: DrawSource<'a>,
    paint: &'a Paint,
    bounds: Rect,
    slot: u32,
}

impl<'a> RecordedDraw<'a> {
    /// `of` is the draw `op` records, pinned to the replay `state`.
    fn of(op: &'a Op, state: &ReplayState) -> Self {
        let (source, paint, bounds, slot) = match op {
            Op::DrawRect {
                rect,
                paint,
                bounds,
                slot,
            } => {
                let source = DrawSource::Shape {
                    shape: Shape::Rect(*rect),
                    ink: *rect,
                };
                (source, paint, state.device_bounds(bounds), slot)
            }
            Op::DrawPath {
                path,
                fill_rule,
                paint,
                content_bounds,
                bounds,
                slot,
            } => {
                let source = DrawSource::Shape {
                    shape: Shape::of_path(path, *fill_rule, &paint.style),
                    ink: *content_bounds,
                };
                (source, paint, state.device_bounds(bounds), slot)
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
                let source = DrawSource::Image {
                    image,
                    src: *src,
                    dst: *dst,
                    sampling: *sampling,
                };
                (source, paint, state.device_bounds(bounds), slot)
            }
            Op::RRectBlur {
                rect,
                radii,
                paint,
                bounds,
                slot,
            } => {
                // The recorder takes the analytic path only for a paint
                // with a mask blur; none is a blur of σ 0, a sharp rrect.
                let source = DrawSource::RRectBlur {
                    rect: *rect,
                    radii: *radii,
                    blur: paint.mask_blur.unwrap_or(MaskBlur::new(0.0)),
                };
                (source, paint, state.device_bounds(bounds), slot)
            }
            Op::GlyphRun {
                font,
                size,
                paint,
                glyphs,
                content_bounds,
                bounds,
                slot,
            } => {
                let device_bounds = state.device_bounds(bounds);
                let source = DrawSource::Glyphs(GlyphRun {
                    font,
                    size: *size,
                    glyphs,
                    content_bounds: *content_bounds,
                    device_bounds,
                    colour_filter: None,
                });
                (source, paint, device_bounds, slot)
            }
            other => unreachable!("replay_list hands over draw ops only, not {other:?}"),
        };
        Self {
            source,
            paint,
            bounds,
            slot: state.absolute(*slot),
        }
    }
}

/// `OpCursor` walks a list's ops in order and can skip a scope whole.
struct OpCursor<'a> {
    ops: &'a [Op],
    next: usize,
}

impl<'a> OpCursor<'a> {
    fn new(ops: &'a [Op]) -> Self {
        Self { ops, next: 0 }
    }

    fn next(&mut self) -> Option<&'a Op> {
        let op = self.ops.get(self.next)?;
        self.next += 1;
        Some(op)
    }

    /// `skip_scope` moves past the scope the last op opened, up to and
    /// including its matching `Restore` (used when a layer is invisible).
    fn skip_scope(&mut self) {
        let mut depth = 1usize;
        while depth > 0 {
            match &self.ops[self.next] {
                Op::Save | Op::SaveLayer { .. } => depth += 1,
                Op::Restore => depth -= 1,
                _ => {}
            }
            self.next += 1;
        }
    }
}
