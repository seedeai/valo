//! Backdrops: a save layer that opens pre-filled with the filtered scene
//! beneath it, the glass its children paint over (Flutter's `saveLayer`
//! with a backdrop). The parent is copied BEFORE the layer's texture opens —
//! the whole point of backdrop-as-a-layer-property: the glass shows the real
//! scene, not a fresh offscreen — and the backdrop's filter tree renders
//! over that copy. Keyed tiles of one group share the first tile's result
//! (Impeller's `shared_filter_snapshot`).

use rustc_hash::FxHashMap;
use valo_dl::{Backdrop, DisplayList, ImageFilter, TileMode};
use valo_geometry::{Matrix, Rect};

use crate::pipelines::PipelineBlend;

use super::draw_state::DrawState;
use super::filter_output::FilterOutput;
use super::filter_tree::{Filter, FilterInput};
use super::gaussian::{basis_of, expanded, BlurBounds, BlurPlan};
use super::layers::round_out;
use super::snapshot::Snapshot;
use super::Planner;

/// `BackdropRequest` is a recorded backdrop pinned to one replay.
///
/// The walk validates the shared key against the list's backdrop groups
/// before the layer opens, so the seed logic never re-checks filter agreement.
pub(super) struct BackdropRequest {
    /// Filter in local units, transformed at the save point.
    pub filter: ImageFilter,
    /// The group whose first tile's result the layer shares; `None` when the
    /// backdrop is unkeyed, or its group's tiles disagree on the filter.
    pub shared: Option<SharedGroup>,
}

/// `SharedGroup` is a backdrop's shared key and its group's union bounds,
/// in the target's coordinates.
pub(super) struct SharedGroup {
    pub key: u64,
    pub bounds: Rect,
}

impl BackdropRequest {
    /// `of` is `backdrop`, recorded in `list`, whose root space `base` maps
    /// into the target's coordinates. Different filters under one key cannot
    /// share a snapshot, so a key whose group disagrees on the filter is
    /// dropped.
    pub fn of(backdrop: &Backdrop, list: &DisplayList, base: &Matrix) -> Self {
        let shared = backdrop
            .shared_key
            .and_then(|key| list.backdrop_group(key))
            .filter(|group| group.filter.is_some())
            .map(|group| SharedGroup {
                key: group.key,
                bounds: base.map_rect(&group.union_bounds),
            });
        Self {
            filter: backdrop.filter.clone(),
            shared,
        }
    }
}

/// `SharedBackdrop` holds one shared key's filtered snapshot.
///
/// Registered by the first tile replayed; later same-key layers in the
/// same target seed from it without another pass break.
pub(super) struct SharedBackdrop {
    pub snapshot: Snapshot,
    /// The filter and the mapping it was planned under: see
    /// [`SharedBackdrop::matches`].
    pub filter: ImageFilter,
    pub local_to_target: Matrix,
    /// The target the blur snapshotted. A same-key tile in a DIFFERENT
    /// target (a materialized layer vs the main target) must not reuse it:
    /// the coords and the pixels both belong to the other texture.
    pub source: wgpu::Texture,
}

impl SharedBackdrop {
    /// `matches` reports whether this snapshot is what `filter` would make of
    /// the same target under `local_to_target`. σ depends only on the
    /// mapping's basis, so tiles that differ by a translation share; a
    /// bounded blur's bounds move with the translation too, so a bounded
    /// filter needs the whole mapping to agree.
    pub fn matches(&self, filter: &ImageFilter, local_to_target: &Matrix) -> bool {
        if self.filter != *filter {
            return false;
        }
        if reads_within_bounds(filter) {
            self.local_to_target == *local_to_target
        } else {
            basis_of(&self.local_to_target) == basis_of(local_to_target)
        }
    }
}

impl Planner<'_> {
    /// `render_backdrop_seed` filters the scene beneath `rect`: the glass the
    /// layer opens with, the layer's composite drawing at `composite`, whose
    /// transform is the filter's. Matching keyed tiles reuse the first
    /// snapshot; the first tile filters the whole group's union, so later
    /// tiles find their part in it. `None` when the filter makes nothing
    /// there.
    pub(super) fn render_backdrop_seed(
        &mut self,
        composite: &DrawState,
        rect: &Rect,
        request: BackdropRequest,
        shared_backdrops: &mut FxHashMap<u64, SharedBackdrop>,
    ) -> Option<FilterOutput<'static>> {
        let effect_transform = &composite.transform;
        if let Some(shared) = request
            .shared
            .as_ref()
            .and_then(|group| shared_backdrops.get(&group.key))
            .filter(|shared| shared.matches(&request.filter, effect_transform))
            .filter(|shared| shared.source == self.contexts.top().src_texture)
        {
            self.plan.stats.shared_backdrops += 1;
            return Some(FilterOutput::Snapshot(shared.snapshot.clone()));
        }
        let area = self.contexts.top().area;
        let hint = request
            .shared
            .as_ref()
            .and_then(|group| group.bounds.intersect(&area.rect()))
            .unwrap_or(*rect);
        let output = self.render_backdrop(&hint, &request.filter, composite)?;
        self.plan.stats.backdrops += 1;
        let Some(SharedGroup { key, .. }) = request.shared else {
            return Some(output);
        };
        // A shared result is read again by later tiles, so it is a texture
        // (Impeller's `shared_filter_snapshot`). First tile wins: a filter-,
        // transform-, or target-mismatched tile blurs independently WITHOUT
        // evicting the entry later matching tiles reuse.
        let snapshot = self.snapshot_output(output);
        shared_backdrops.entry(key).or_insert(SharedBackdrop {
            snapshot: snapshot.clone(),
            filter: request.filter,
            local_to_target: *effect_transform,
            source: self.contexts.top().src_texture.clone(),
        });
        Some(FilterOutput::Snapshot(snapshot))
    }

    /// `draw_backdrop_seed` draws the filtered parent into the just-opened
    /// layer as its FIRST step — the glass every child paints over, drawn
    /// by the backdrop tree's last node, at the depth floor below every
    /// child. The seed is in replay coordinates, so the layer's origin shift
    /// places it.
    pub(super) fn draw_backdrop_seed(&mut self, seed: &FilterOutput<'_>) {
        let floor = self.contexts.top().depth.floor();
        self.drawing().draw_filter_output(
            seed,
            PipelineBlend::SrcOver,
            &DrawState::in_layer(Matrix::IDENTITY, floor),
        );
    }

    /// `render_backdrop` copies the parent before the layer opens, then
    /// renders the backdrop's filter tree over that copy. `hint` is what the
    /// backdrop has to cover, in replay coordinates (Impeller's coverage
    /// hint: the new layer); `composite`'s transform maps the filter's local
    /// coordinates into replay coordinates, and the parent draws on after
    /// its depth. `None` when the filter makes nothing there.
    pub(super) fn render_backdrop(
        &mut self,
        hint: &Rect,
        filter: &ImageFilter,
        composite: &DrawState,
    ) -> Option<FilterOutput<'static>> {
        let local_to_target = &composite.transform;
        let parent = self.split_for_copy(composite.z);
        // An unspecified blur on a backdrop mirrors, as dart:ui's
        // `pushBackdropFilter` gives it.
        let tree = FilterInput::Source.with_image_filter(Some(filter), TileMode::Mirror);
        match backdrop_blur(&tree, &parent, hint, local_to_target) {
            Some(blur) => {
                // One blur reads the parent itself, as Impeller's backdrop
                // blur reads its backdrop texture.
                let blurred = self.filter_passes().push_blur(&blur, &parent.view);
                Some(FilterOutput::Snapshot(blurred))
            }
            None => self.render_parent_filters(&parent, hint, &tree, local_to_target),
        }
    }

    /// `render_parent_filters` is any other backdrop: it reads a copy of
    /// what its tree needs of the parent for the hint, cut at whole texels so
    /// the copy is the parent's pixels as they are, and a blur in it reads
    /// past that copy's edge as its clamped edge.
    fn render_parent_filters(
        &mut self,
        parent: &Snapshot,
        hint: &Rect,
        tree: &FilterInput<'static>,
        local_to_target: &Matrix,
    ) -> Option<FilterOutput<'static>> {
        let placement = &parent.placement;
        let needed = tree
            .source_coverage(local_to_target, hint)
            .intersect(&placement.coverage())?;
        let region = placement
            .transform
            .map_rect(&round_out(&placement.texel_rect(&needed)));
        let source = self.filter_passes().push_resample(parent, &region);
        Some(self.render_filter_tree(tree, &source, local_to_target, None))
    }
}

/// `backdrop_blur` plans the blur of a backdrop tree that is one blur over
/// the parent, the case that reads the parent itself; `None` for any other
/// tree or a negligible blur. Where the parent holds `hint` grown by the
/// blur's padding (Impeller's expanded coverage hint), the blur is cut to
/// it; at the parent's edge it blurs the whole parent, read past its edge
/// as the tile mode says.
fn backdrop_blur(
    tree: &FilterInput<'_>,
    parent: &Snapshot,
    hint: &Rect,
    local_to_target: &Matrix,
) -> Option<BlurPlan> {
    let FilterInput::Filter(node) = tree else {
        return None;
    };
    let (
        Filter::Blur {
            bounds, tile_mode, ..
        },
        FilterInput::Source,
    ) = (&node.filter, &node.input)
    else {
        return None;
    };
    let blur = node.filter.blur(local_to_target)?;
    let input_hint = expanded(hint, blur.local_padding());
    let bounds = bounds.as_ref().map(|rect| BlurBounds {
        rect,
        local_to_tree: local_to_target,
    });
    BlurPlan::new(
        &blur,
        &parent.placement,
        Some(&input_hint),
        *tile_mode,
        bounds,
    )
}

/// `reads_within_bounds` reports whether any blur in `filter` is bounded.
fn reads_within_bounds(filter: &ImageFilter) -> bool {
    match filter {
        ImageFilter::Blur { bounds, .. } => bounds.is_some(),
        ImageFilter::Color(_) | ImageFilter::DropShadow { .. } => false,
        ImageFilter::Compose { outer, inner } => {
            reads_within_bounds(outer) || reads_within_bounds(inner)
        }
    }
}
