//! Segments: a context's pending draws becoming a pass, and the split that
//! ends a pass so what the context holds so far can be copied — for a
//! destination read or a backdrop — before the next pass draws over it.
//!
//! Every pass starts fresh (Skia Graphite, Impeller): its multisample
//! colour, depth and stencil are cleared, and nothing of them survives the
//! pass. So a split copies the whole of what the context resolved, and the
//! next pass draws that copy back first and replays the clips still active,
//! as Impeller's `FlipBackdrop` draws its backdrop back and replays its clip
//! stack's entities. The pass list itself is the plan writer's.

use valo_geometry::{Color, Matrix};

use crate::frame::{Draw, TextureCopy};
use crate::pipelines::{PipelineBlend, PipelineKind};

use super::draw_state::DrawState;
use super::filter_output::FilterOutput;
use super::snapshot::Snapshot;
use super::Planner;

impl Planner<'_> {
    /// `emit_segment` flushes the top context's pending draws into a pass.
    /// A context's first pass clears to its clear colour, whatever it
    /// draws; a later one clears to transparent, under the draw-back that
    /// opens it, and is skipped when it draws nothing.
    pub(super) fn emit_segment(&mut self) {
        let context = self.contexts.top_mut();
        let first = !context.first_pass_emitted;
        if !first && context.draws.is_empty() {
            return;
        }
        context.first_pass_emitted = true;
        let clear = if first {
            context.clear
        } else {
            Color::TRANSPARENT
        };
        let draws = std::mem::take(&mut context.draws);
        let attachments = context.attachments.clone();
        self.plan.push_context_pass(attachments, clear, draws);
    }

    /// `split_for_copy` ends the top context's pass and returns a copy of
    /// what it holds, for a reader at depth `reader_z` to sample: the next
    /// pass starts with the copy drawn back and the clips that still hold
    /// past `reader_z` replayed.
    pub(super) fn split_for_copy(&mut self, reader_z: f32) -> Snapshot {
        self.plan.stats.snapshots += 1;
        self.emit_segment();
        let copy = self.draw_back();
        self.replay_clips(reader_z);
        copy
    }

    /// `start_from_existing_pixels` starts the top context from what its
    /// texture shows before anything draws: its pixels are copied out and
    /// drawn back as its first draw, under every later one. Multisample
    /// scratch never keeps a picture between passes, so this is how a target
    /// drawn over without clearing keeps its pixels: Skia's load from
    /// resolve, and Impeller's draw-back of a target it resumes.
    pub(super) fn start_from_existing_pixels(&mut self) {
        self.draw_back();
    }

    /// `draw_back` copies the whole of what the top context's texture holds
    /// before the next pass, and draws the copy back as that pass's first
    /// draw, at the depth floor; it returns the copy, which lies where the
    /// context's texels do.
    fn draw_back(&mut self) -> Snapshot {
        let context = self.contexts.top();
        let copy = self
            .tools
            .pool
            .take_copy_texture(context.area.size(), self.tools.format);
        self.plan.queue_copy(TextureCopy {
            src: context.src_texture.clone(),
            dst: copy.texture,
            size: context.area.size(),
        });
        let picture = Snapshot {
            view: copy.view,
            placement: context.area.placement(),
        }
        .placed_where_drawn(true);
        let floor = context.depth.floor();
        self.drawing().draw_filter_output(
            &FilterOutput::Snapshot(picture.clone()),
            PipelineBlend::SrcOver,
            &DrawState::in_layer(Matrix::IDENTITY, floor),
        );
        picture
    }

    /// `replay_clips` draws again the top context's clips whose ceilings
    /// hold past `reader_z`, after the draw-back, and forgets the rest: no
    /// later draw is beneath them.
    fn replay_clips(&mut self, reader_z: f32) {
        let context = self.contexts.top_mut();
        context.clips.retain(|clip| clip.z > reader_z);
        let replayed = context.clips.clone();
        context.draws.extend(replayed);
    }
}

/// `reorder_draws` is Impeller's DrawOrderResolver, as a pure pass over one
/// pass's draws: clips are BARRIERS, and between barriers opaque draws go
/// first, front-to-back — painter order among the rest is untouched.
/// Hoisting never crosses a barrier (a clip ceiling must be in the depth
/// buffer before the draws it scopes). A draw moves whole, its stencil with
/// it.
pub(super) fn reorder_draws(draws: Vec<Draw>, hoisted: &mut u32) -> Vec<Draw> {
    let mut out = Vec::with_capacity(draws.len());
    let mut chunk: Vec<Draw> = Vec::new(); // draws between barriers
    for draw in draws {
        if matches!(draw.key.kind, PipelineKind::ClipCover { .. }) {
            flush_chunk(&mut chunk, &mut out, hoisted);
            out.push(draw);
        } else {
            chunk.push(draw);
        }
    }
    flush_chunk(&mut chunk, &mut out, hoisted);
    out
}

/// `flush_chunk` emits one barrier-free chunk: opaque draws first (z
/// descending = front to back), everything else in painter order.
fn flush_chunk(chunk: &mut Vec<Draw>, out: &mut Vec<Draw>, hoisted: &mut u32) {
    let is_opaque = |draw: &Draw| {
        matches!(
            draw.key.kind,
            PipelineKind::OpaqueDraw(_) | PipelineKind::OpaqueCover(_)
        )
    };
    let mut seen_blended = false;
    let mut opaque: Vec<Draw> = Vec::new();
    let mut blended: Vec<Draw> = Vec::new();
    for draw in chunk.drain(..) {
        if is_opaque(&draw) {
            if seen_blended {
                *hoisted += 1; // drawn out of painter order
            }
            opaque.push(draw);
        } else {
            seen_blended = true;
            blended.push(draw);
        }
    }
    opaque.sort_by(|a, b| b.z.total_cmp(&a.z));
    out.extend(opaque);
    out.extend(blended);
}

/// The reorder moves whole draws: a path's stencil goes with its cover, and
/// nothing crosses a clip.
#[cfg(test)]
mod tests {
    use super::*;
    use crate::frame::{Bindings, Geometry, Mesh, Stencil};
    use crate::host_buffer::{UniformSlot, VertexSlot};
    use crate::pipelines::{Frag, PipelineBlend, PipelineKey};

    const FORMAT: wgpu::TextureFormat = wgpu::TextureFormat::Rgba8Unorm;

    fn draw(kind: PipelineKind, z: f32, stencilled: bool) -> Draw {
        let uniforms = UniformSlot {
            block: 0,
            offset: (z * 1000.0) as u32,
        };
        let stencil = stencilled.then(|| Stencil {
            key: PipelineKey::new(
                FORMAT,
                PipelineBlend::SrcOver,
                PipelineKind::StencilFan { even_odd: false },
            ),
            uniforms,
            mesh: Mesh {
                slot: VertexSlot {
                    block: 0,
                    offset: 0,
                    bytes: 24,
                },
                vertices: 3,
            },
        });
        Draw {
            stencil,
            key: PipelineKey::new(FORMAT, PipelineBlend::SrcOver, kind),
            uniforms,
            bindings: Bindings::Plain,
            geometry: Geometry::Quad,
            z,
        }
    }

    /// The draws' depths and whether each carries its stencil, in order.
    fn order(draws: &[Draw]) -> Vec<(f32, bool)> {
        draws
            .iter()
            .map(|draw| (draw.z, draw.stencil.is_some()))
            .collect()
    }

    #[test]
    fn opaque_draws_go_first_front_to_back_with_their_stencils() {
        let draws = vec![
            draw(PipelineKind::Cover(Frag::Solid), 0.1, true),
            draw(PipelineKind::OpaqueCover(Frag::Solid), 0.2, true),
            draw(PipelineKind::OpaqueDraw(Frag::Solid), 0.3, false),
        ];
        let mut hoisted = 0;
        let reordered = reorder_draws(draws, &mut hoisted);
        assert_eq!(order(&reordered), [(0.3, false), (0.2, true), (0.1, true)]);
        assert_eq!(hoisted, 2);
    }

    #[test]
    fn nothing_crosses_a_clip() {
        let draws = vec![
            draw(PipelineKind::Draw(Frag::Solid), 0.1, false),
            draw(PipelineKind::ClipCover { difference: false }, 0.5, true),
            draw(PipelineKind::OpaqueDraw(Frag::Solid), 0.3, false),
        ];
        let mut hoisted = 0;
        let reordered = reorder_draws(draws, &mut hoisted);
        assert_eq!(order(&reordered), [(0.1, false), (0.5, true), (0.3, false)]);
        assert_eq!(hoisted, 0);
    }
}
