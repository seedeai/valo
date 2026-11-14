//! A paint shader's effect as an image filter, by Flutter's convention:
//! Impeller's `RuntimeEffectFilterContents`. The filter's input is the
//! effect's first image, the input's size in pixels its first uniform, and
//! `shade` runs at positions in the input's pixels: the quad drawn is the
//! input's texels, placed by the input's own transform, as Impeller draws
//! `Rect::MakeSize(texture size)` under the snapshot's transform. So the
//! effect's paint-shader module serves unchanged, and what the filter
//! covers is what its input covers.
//!
//! The invariant a later editor will break: `shade`'s positions are the
//! input texture's pixels, so an input whose texels are scaled against the
//! tree's space (a blur's smaller result, an image drawn larger) is first
//! drawn again at one texel per pixel, as Impeller re-rasterizes a snapshot
//! whose transform scales (`ShouldRasterizeForRuntimeEffects`).

use std::sync::Arc;

use valo_dl::RuntimeShader;
use valo_geometry::{Matrix, Rect};

use super::draw_context::PixelArea;
use super::filter_output::FilterOutput;
use super::filter_passes::FilterPasses;
use super::snapshot::Snapshot;
use super::Planner;

/// `ShaderFilterOutput` is what a paint shader's effect draws as a filter:
/// `shade` over `region` of the tree's space, reading `input` as its first
/// image.
pub(super) struct ShaderFilterOutput {
    /// The draw's inputs, its first uniform the input's size.
    pub shader: RuntimeShader,
    /// The filter's input, at one texel per pixel of the tree's space.
    pub input: Snapshot,
    pub region: Rect,
}

impl ShaderFilterOutput {
    /// `texel_quad` is what the filter shades, in the input's texels: the
    /// local space of its draw.
    pub fn texel_quad(&self) -> Rect {
        let placement = &self.input.placement;
        let texels = Rect::new(0.0, 0.0, placement.size[0] as f32, placement.size[1] as f32);
        let quad = placement.texel_rect(&self.region);
        quad.intersect(&texels).unwrap_or(quad)
    }
}

impl Planner<'_> {
    /// `render_shader_filter` is Impeller's
    /// `RuntimeEffectFilterContents::RenderFilter` over `input`, shading
    /// what `hint` asks for of what the input covers.
    pub(super) fn render_shader_filter(
        &mut self,
        shader: &RuntimeShader,
        input: Snapshot,
        hint: Option<Rect>,
    ) -> FilterOutput<'static> {
        let input = self.unscaled_input(input);
        let coverage = input.placement.coverage();
        // A hint the coverage misses cuts nothing, as a fill's does.
        let region = hint
            .and_then(|hint| hint.intersect(&coverage))
            .unwrap_or(coverage);
        FilterOutput::ShaderFilter(ShaderFilterOutput {
            shader: with_input_size(shader, input.placement.size),
            input,
            region,
        })
    }

    /// `unscaled_input` is `input` at one texel per pixel: as it is when its
    /// transform only moves it or turns it, as Impeller keeps a turned one,
    /// or drawn again over what it covers.
    fn unscaled_input(&mut self, input: Snapshot) -> Snapshot {
        let [a, b, c, d, _, _] = input.placement.transform.to_affine();
        let turns = b != 0.0 || c != 0.0;
        if turns || (a == 1.0 && d == 1.0) {
            return input;
        }
        let coverage = input.placement.coverage();
        self.filter_passes().push_resample(&input, &coverage)
    }
}

impl FilterPasses<'_, '_> {
    /// `push_shader_filter` renders a paint shader's effect drawn as a
    /// filter into a texture of its region.
    pub fn push_shader_filter(&mut self, output: &ShaderFilterOutput) -> Snapshot {
        let area = PixelArea::over(output.region);
        let origin = area.origin();
        let texels_to_pass =
            Matrix::translation(-origin.x, -origin.y).then(&output.input.placement.transform);
        let target = self.pool.take_filter(area.size(), self.format);
        let shading = self
            .emit
            .shader_filter_shading(&output.shader, &output.input.view, 1.0);
        let draw = self.emit.mapped_filter_draw(
            self.format,
            &texels_to_pass,
            &output.texel_quad(),
            area.size(),
            shading,
        );
        self.plan.push_filter_pass(target.view.clone(), draw);
        Snapshot {
            view: target.view,
            placement: area.placement(),
        }
    }
}

/// `with_input_size` is `shader` with its first uniform, a `vec2<f32>`, set
/// to the input's size in pixels, as Impeller writes it over the uniforms'
/// first eight bytes.
fn with_input_size(shader: &RuntimeShader, size: [u32; 2]) -> RuntimeShader {
    let mut uniforms = shader.uniforms.to_vec();
    let [width, height] = size.map(|length| length as f32);
    uniforms[0..4].copy_from_slice(&width.to_ne_bytes());
    uniforms[4..8].copy_from_slice(&height.to_ne_bytes());
    RuntimeShader {
        uniforms: Arc::from(uniforms),
        ..shader.clone()
    }
}
