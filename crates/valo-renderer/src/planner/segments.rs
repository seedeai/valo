//! Pass emission: flushing an open frame's accumulated steps into the plan,
//! and the dst-read break that splits a target into segments around a
//! snapshot. This module is the only writer of [`FramePlan`]'s pass list.

use valo_geometry::{Point, Rect};

use crate::frame::{replace_msaa, PassColor, PlannedPass, Step, TextureCopy};
use crate::pipelines::PipelineKind;

use super::Planner;

impl Planner<'_> {
    /// `emit_segment` flushes the current frame's steps into a
    /// [`PlannedPass`].
    ///
    /// Empty resumed segments with no pending copies are skipped. The first
    /// segment of a target carries its clear; later ones Load. Opaque draws
    /// are reordered before the pass is pushed, and the previous segment's
    /// `store` flag is set so the MSAA scratch survives until this one.
    pub(super) fn emit_segment(&mut self) {
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

    /// `break_pass` ends the current segment and schedules a dst snapshot:
    /// the NEXT segment starts by copying the target's resolved contents
    /// under `coverage` (the dst-reading draw's device bounds — the only
    /// pixels it can sample).
    pub(super) fn break_pass(&mut self, coverage: &Rect) -> wgpu::TextureView {
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

    /// `swap_to_persistent_attachments` rescues a target that turned out to
    /// need stores: a dst-reading break splits it into segments, and
    /// tile-only attachments cannot store — swap in a persistent msaa +
    /// depth pair, both for the segments to come and for the one already
    /// emitted (planning-time swap: nothing has rendered yet). The resolve
    /// — where snapshots copy from — stays.
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
}

/// `snapshot_region` is the frame-local integer pixel region under
/// `coverage` (absolute replay coords), rounded outward and clamped —
/// `None` when nothing visible needs copying. Fragments only exist inside
/// their draw's bounds, so this region is every texel a dst read can touch.
pub(super) fn snapshot_region(
    coverage: &Rect,
    origin: Point,
    size: [u32; 2],
) -> Option<([u32; 2], [u32; 2])> {
    let x0 = (coverage.x - origin.x).floor().max(0.0) as u32;
    let y0 = (coverage.y - origin.y).floor().max(0.0) as u32;
    let x1 = ((coverage.x + coverage.width - origin.x).ceil().max(0.0) as u32).min(size[0]);
    let y1 = ((coverage.y + coverage.height - origin.y).ceil().max(0.0) as u32).min(size[1]);
    (x1 > x0 && y1 > y0).then_some(([x0, y0], [x1 - x0, y1 - y0]))
}

/// `reorder_segment` is Impeller's DrawOrderResolver, as a pure pass over
/// one segment: fans glue to the step that follows them (a draw unit), clip
/// units are BARRIERS, and between barriers opaque units draw first,
/// front-to-back — painter order among the rest is untouched. Hoisting
/// never crosses a barrier (a clip ceiling must be in the depth buffer
/// before the draws it scopes).
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

/// `flush_chunk` emits one barrier-free chunk: opaque units first (z
/// descending = front to back), everything else in painter order.
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
