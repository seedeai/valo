//! An embedded list the host hinted as cacheable: drawn as one quad that
//! samples the list's cached raster, the raster filled this frame when the
//! cache asks for it, or the list replayed inline when it cannot be cached
//! under the embed's transform.
//!
//! A fill renders as passes of its own before the quad that samples it —
//! exactly how every save layer renders before the composite that samples
//! it — so a fill never changes the pixels the user is looking at. A fill's
//! walk replays any cacheable embed inside it inline, so one fill never
//! schedules another and the cached pixels stay a plain rendering of the
//! list.

use std::sync::Arc;

use valo_dl::DisplayList;
use valo_geometry::{Matrix, Rect};

use crate::pipelines::PipelineBlend;
use crate::raster::{FillTarget, QuadSource, RasterVerdict};

use super::draw_state::DrawState;
use super::drawing::Drawing;
use super::emit::{Cover, Entity, Role};
use super::replay::ReplayState;
use super::Planner;

impl Planner<'_> {
    /// `embed_cached_list` is a hinted embed: sample the cached raster as
    /// one quad, or fall back to inline replay — scheduling a fill when the
    /// cache asks for one.
    pub(super) fn embed_cached_list(
        &mut self,
        list: &Arc<DisplayList>,
        base_slot: u32,
        state: &ReplayState,
    ) {
        let at = self.draw_state(state, base_slot);
        if !embeds_as_quad(&at.transform) {
            return self.replay_embedded(list, base_slot, state);
        }
        let device = self.tools.device;
        let verdict = self.caches.rasters.resolve(
            device,
            self.tools.format,
            list,
            at.transform.max_scale(),
            device.limits().max_texture_dimension_2d,
        );
        match verdict {
            RasterVerdict::Quad(source) => self.drawing().draw_raster_quad(&source, &at),
            RasterVerdict::Fill(target) => {
                let source = target.quad_source();
                self.render_raster_fill(list, target);
                self.drawing().draw_raster_quad(&source, &at);
            }
            RasterVerdict::Inline => self.replay_embedded(list, base_slot, state),
        }
    }

    /// `render_raster_fill` renders one cache entry mid-walk, into the
    /// cache's own texture. The cached pixels are the list on its own: its
    /// slots start at zero in the texture's depth space, and an enclosing
    /// elided group's alpha belongs to the quad, not to what the texture
    /// holds.
    fn render_raster_fill(&mut self, list: &Arc<DisplayList>, fill: FillTarget) {
        self.plan.stats.raster_fills += 1;
        // p_texture = scale · (p_list − origin): the translate applies first.
        let base = Matrix::scale(fill.content_scale, fill.content_scale).then(
            &Matrix::translation(-fill.content_bounds.x, -fill.content_bounds.y),
        );
        self.open_raster_target(&fill, list);
        self.replay_list(list, &mut ReplayState::raster_fill(base));
        self.close_target();
    }
}

impl Drawing<'_, '_> {
    /// `draw_raster_quad` draws one sampled quad standing in for a whole
    /// cached sub-list, at `at`'s transform, depth and group alpha (Flutter
    /// draws a raster-cached picture with the inherited opacity:
    /// `DisplayListLayer::Paint`). At (near-)exact scale
    /// the origin snaps to integral device px and the destination takes the
    /// texture's own integer size, which is what makes it texel-perfect
    /// against inline replay (Flutter's `GetIntegralTransCTM` discipline).
    fn draw_raster_quad(&mut self, source: &QuadSource, at: &DrawState) {
        self.stats.raster_quads += 1;
        let embed = &at.transform;
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
        // UVs map dest coords onto the FULL texture, which is ceil-sized
        // past the content the way layer textures are — the same
        // convention every composite uses.
        let sampled = Rect::new(dest.x, dest.y, extent[0], extent[1]);
        let entity = Entity {
            stencil: None,
            cover: Cover::Quad {
                transform: Matrix::IDENTITY,
                rect: dest,
            },
            role: Role::Fill,
            shading: self.emit.raster_shading(&source.view, &sampled, at.alpha),
            blend: PipelineBlend::SrcOver,
            z: at.z,
        };
        self.emit.push(self.context, entity);
    }
}

/// `embeds_as_quad` reports whether a cached raster can stand in for a list
/// under `embed`. The composite quad is axis-aligned in pass coords, so
/// rotated or skewed embeds replay inline instead (Flutter skips integral
/// snapping under complex transforms for the same reason — flutter#41654).
fn embeds_as_quad(embed: &Matrix) -> bool {
    let [_, shear_b, shear_c, ..] = embed.to_affine();
    shear_b == 0.0 && shear_c == 0.0 && embed.is_affine()
}
