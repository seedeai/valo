//! What a draw's fragment reads, made together: its [`Shading`] — the
//! fragment, its uniform record's tint and payload, and its bindings — so a
//! fragment and what it reads cannot disagree. There is one encoder per
//! fragment family; the payload slots and ids are `shader_abi`'s, and the
//! record's transform is written when the draw is placed (`emit`).

use valo_dl::{
    ColorFilter, FocalCircle, GradientStop, Image, MaskBlur, MaskKind, Paint, Sampling, Shader,
    SpreadMode, TileMode, MAX_GRADIENT_STOPS,
};
use valo_geometry::{Color, Matrix, Point, Rect};

use crate::frame::Bindings;
use crate::pipelines::{AdvancedBlend, Blend, Frag};
use crate::shader_abi::{
    blur_style_id, conical, gamma, mask_kind_id, slot, spread_id, UniformRecord,
};

use super::emit::StepEmitter;
use super::filter_output::Merge;
use super::gaussian::{BlurPass, DownsampleTaps};
use super::snapshot::Snapshot;

/// `Shading` is how a draw's pixels are coloured: its fragment, the
/// record it reads, and what it reads besides.
pub(super) struct Shading {
    pub frag: Frag,
    pub record: UniformRecord,
    pub bindings: Bindings,
    /// The colour covers every pixel it touches with full alpha: an opaque
    /// fill may write its depth and be drawn early.
    pub opaque: bool,
}

impl Shading {
    /// `none` is the shading of a draw that writes no colour: a clip.
    pub fn none() -> Self {
        Self::of(
            Frag::Solid,
            UniformRecord::tinted([0.0; 4]),
            Bindings::Plain,
        )
    }

    /// `solid` is one premultiplied colour.
    pub fn solid(tint: [f32; 4]) -> Self {
        Self::of(Frag::Solid, UniformRecord::tinted(tint), Bindings::Plain)
    }

    /// `rrect_blur` is the closed-form blurred (r)rect: coverage evaluated
    /// analytically over `rect` with per-corner `radii`, `blur`'s σ and
    /// style, in `color` at `alpha`.
    pub fn rrect_blur(
        rect: &Rect,
        radii: [f32; 4],
        color: Color,
        blur: MaskBlur,
        alpha: f32,
    ) -> Self {
        let mut record = UniformRecord::tinted(scaled_premul(color, alpha));
        record.set(slot::GEOM, [rect.x, rect.y, rect.right(), rect.bottom()]);
        record.set(
            slot::MISC,
            [blur.sigma.max(0.05), blur_style_id(blur.style), 0.0, 0.0],
        );
        record.set(slot::RADII, radii);
        Self::of(Frag::RRectBlur, record, Bindings::Plain)
    }

    /// `glyphs` is one atlas page's glyphs, through `frag`, tinted `tint`.
    pub fn glyphs(frag: Frag, tint: [f32; 4], page: wgpu::BindGroup) -> Self {
        Self::of(frag, UniformRecord::tinted(tint), Bindings::Textured(page))
    }

    /// `colour_glyphs` is colour glyphs (emoji) from `page`: their own
    /// pixels at `alpha`, through `filter` when there is one (Skia filters
    /// a colour glyph's pixel at the paint's alpha), then at `group_alpha`.
    pub fn colour_glyphs(
        page: wgpu::BindGroup,
        filter: Option<ColorFilter>,
        alpha: f32,
        group_alpha: f32,
    ) -> Self {
        let Some(filter) = filter else {
            return Self::glyphs(Frag::GlyphColor, alpha_tint(alpha * group_alpha), page);
        };
        let mut record = UniformRecord::tinted(alpha_tint(group_alpha));
        let frag = encode_color_filter(&mut record, filter, alpha).glyph_fragment();
        Self::of(frag, record, Bindings::Textured(page))
    }

    fn of(frag: Frag, record: UniformRecord, bindings: Bindings) -> Self {
        Self {
            frag,
            record,
            bindings,
            opaque: false,
        }
    }
}

impl StepEmitter<'_> {
    /// `paint_shading` is a paint's colour or shader at `alpha` times its
    /// own: a solid, a gradient (its stops in the record, or baked into a
    /// ramp past the record's budget), or a pattern. It is opaque when the
    /// paint covers every pixel it touches.
    pub fn paint_shading(&mut self, paint: &Paint, alpha: f32) -> Shading {
        let mut record = UniformRecord::tinted(tinted(paint, alpha));
        let (frag, bindings) = match PaintSource::of(paint) {
            PaintSource::Solid => (Frag::Solid, Bindings::Plain),
            PaintSource::Pattern {
                image,
                sampling,
                local,
            } => {
                fill_pattern_payload(&mut record, image, sampling, local);
                (
                    Frag::Pattern,
                    Bindings::Textured(self.image_bind(image, sampling)),
                )
            }
            PaintSource::Gradient(gradient) if gradient.stops.len() > MAX_GRADIENT_STOPS => {
                let (ramp, texels) = self.ramp(gradient.stops);
                fill_gradient_payload(&mut record, &gradient, Some(texels));
                (gradient.ramp_frag(), Bindings::Textured(ramp))
            }
            PaintSource::Gradient(gradient) => {
                fill_gradient_payload(&mut record, &gradient, None);
                (gradient.frag(), Bindings::Plain)
            }
        };
        Shading {
            opaque: alpha >= 1.0 && is_opaque_paint(paint),
            ..Shading::of(frag, record, bindings)
        }
    }

    /// `image_shading` is `src` of `image` sampled over `dst` at `alpha`,
    /// through `color_filter` on each sample (Impeller's atlas path). Tints
    /// with ALPHA only — the image shader multiplies samples by its tint,
    /// and the default paint colour is black.
    pub fn image_shading(
        &mut self,
        image: &Image,
        src: &Rect,
        dst: &Rect,
        sampling: Sampling,
        color_filter: Option<ColorFilter>,
        alpha: f32,
    ) -> Shading {
        let mut record = UniformRecord::tinted(alpha_tint(alpha));
        record.set(slot::GEOM, image_uv(image, src, dst));
        record.set(slot::DECAL, decal_flags(sampling));
        let frag = match color_filter {
            None => Frag::Image,
            Some(filter) => encode_color_filter(&mut record, filter, 1.0).image_fragment(),
        };
        Shading::of(
            frag,
            record,
            Bindings::Textured(self.image_bind(image, sampling)),
        )
    }

    /// `texture_shading` is `snapshot` drawn as it is over what it covers,
    /// at `alpha`.
    pub fn texture_shading(&self, snapshot: &Snapshot, alpha: f32) -> Shading {
        let mut record = UniformRecord::tinted(alpha_tint(alpha));
        record.set(slot::GEOM, snapshot.placement.uv_mapping());
        Shading::of(Frag::Image, record, self.textured(&snapshot.view))
    }

    /// `raster_shading` is a cached list's whole texture stretched over
    /// `sampled`, a rect the texture's size in destination units, at
    /// `alpha`: one premultiplied composite, exactly like a layer's.
    pub fn raster_shading(&self, view: &wgpu::TextureView, sampled: &Rect, alpha: f32) -> Shading {
        let mut record = UniformRecord::tinted(alpha_tint(alpha));
        record.set(slot::GEOM, full_rect_uv(sampled));
        Shading::of(Frag::Image, record, self.textured(view))
    }

    /// `recoloured_shading` is `input` drawn through `filter`, `alpha`
    /// taken in before the filter as Impeller's colour filters absorb a
    /// layer's opacity.
    pub fn recoloured_shading(&self, input: &Snapshot, filter: ColorFilter, alpha: f32) -> Shading {
        let mut record = UniformRecord::tinted([1.0; 4]);
        record.set(slot::GEOM, input.placement.uv_mapping());
        let frag = encode_color_filter(&mut record, filter, alpha).image_fragment();
        Shading::of(frag, record, self.textured(&input.view))
    }

    /// `merged_shading` is two snapshots merged in one fragment, at
    /// `alpha`.
    pub fn merged_shading(
        &self,
        first: &Snapshot,
        second: &Snapshot,
        merge: Merge,
        alpha: f32,
    ) -> Shading {
        let mut record = UniformRecord::tinted(alpha_tint(alpha));
        let frag = encode_merge(
            &mut record,
            first.placement.uv_mapping(),
            second.placement.uv_mapping(),
            merge,
        );
        let bindings = Bindings::Textured(self.blend_bind(&first.view, &second.view));
        Shading::of(frag, record, bindings)
    }

    /// `mask_composite_shading` samples a `mask` layer as coverage, its
    /// luminance or alpha as `kind` says, at `alpha`.
    pub fn mask_composite_shading(&self, mask: &Snapshot, kind: MaskKind, alpha: f32) -> Shading {
        let mut record = UniformRecord::tinted(alpha_tint(alpha));
        record.set(slot::GEOM, mask.placement.uv_mapping());
        record.set(slot::MISC, [mask_kind_id(kind), 0.0, 0.0, 0.0]);
        Shading::of(Frag::MaskComposite, record, self.textured(&mask.view))
    }

    /// `blended_solid_shading` is a solid `color` at `alpha` blended by
    /// `mode` in the fragment against `destination`, a copy of a target
    /// `size` texels big: the result replaces what the copy captured.
    pub fn blended_solid_shading(
        &self,
        color: Color,
        alpha: f32,
        mode: AdvancedBlend,
        destination: &wgpu::TextureView,
        size: [u32; 2],
    ) -> Shading {
        let mut record = UniformRecord::tinted(scaled_premul(color, alpha));
        set_destination_read(&mut record, mode, size);
        Shading::of(Frag::BlendSolid, record, self.textured(destination))
    }

    /// `blended_texture_shading` is `source` at `alpha` blended by `mode`
    /// in the fragment against `destination`: an advanced blend's
    /// composite.
    pub fn blended_texture_shading(
        &self,
        source: &Snapshot,
        alpha: f32,
        mode: AdvancedBlend,
        destination: &wgpu::TextureView,
        size: [u32; 2],
    ) -> Shading {
        let mut record = UniformRecord::tinted(alpha_tint(alpha));
        record.set(slot::GEOM, source.placement.uv_mapping());
        set_destination_read(&mut record, mode, size);
        let bindings = Bindings::Textured(self.blend_bind(destination, &source.view));
        Shading::of(Frag::BlendTexture, record, bindings)
    }

    /// `resample_shading` is a filter pass copying what `source` holds of
    /// `region`, one bilinear tap per pixel.
    pub fn resample_shading(&self, source: &Snapshot, region: &Rect) -> Shading {
        let mut record = UniformRecord::tinted([1.0; 4]);
        let texel = source.placement.size.map(|size| 1.0 / size as f32);
        let uv = source.placement.region_uv_mapping(region);
        encode_downsample(
            &mut record,
            uv,
            DownsampleKernel::ONE_TAP,
            texel,
            None,
            false,
        );
        Shading::of(Frag::Downsample, record, self.textured(&source.view))
    }

    /// `recolour_pass_shading` is a filter pass recolouring what `source`
    /// holds of `region`, `input_alpha` taken in before the filter.
    pub fn recolour_pass_shading(
        &self,
        source: &Snapshot,
        region: &Rect,
        filter: ColorFilter,
        input_alpha: f32,
    ) -> Shading {
        let mut record = UniformRecord::tinted([1.0; 4]);
        record.set(slot::GEOM, source.placement.region_uv_mapping(region));
        let frag = encode_color_filter(&mut record, filter, input_alpha).pass_fragment();
        Shading::of(frag, record, self.textured(&source.view))
    }

    /// `merge_pass_shading` is a filter pass merging what `first` and
    /// `second` hold of `region`.
    pub fn merge_pass_shading(
        &self,
        first: &Snapshot,
        second: &Snapshot,
        region: &Rect,
        merge: Merge,
    ) -> Shading {
        let mut record = UniformRecord::tinted([1.0; 4]);
        let frag = encode_merge(
            &mut record,
            first.placement.region_uv_mapping(region),
            second.placement.region_uv_mapping(region),
            merge,
        );
        let bindings = Bindings::Textured(self.blend_bind(&first.view, &second.view));
        Shading::of(frag, record, bindings)
    }

    /// `bake_shading` is a filter pass recolouring the whole of `source`
    /// through `filter`, into a copy of its size.
    pub fn bake_shading(&self, source: &Image, filter: ColorFilter) -> Shading {
        let size = source.size();
        let mut record = UniformRecord::tinted([1.0; 4]);
        record.set(
            slot::GEOM,
            [1.0 / size[0] as f32, 1.0 / size[1] as f32, 0.0, 0.0],
        );
        let frag = encode_color_filter(&mut record, filter, 1.0).pass_fragment();
        Shading::of(frag, record, self.textured(source.view()))
    }

    /// `downsample_shading` is a blur's downsample reading `input` as
    /// `taps` say.
    pub fn downsample_shading(&self, taps: &DownsampleTaps, input: &wgpu::TextureView) -> Shading {
        let mut record = UniformRecord::tinted([1.0; 4]);
        let edges = taps.edges.map(|edges| edges.0);
        let decal = taps.tile_mode == TileMode::Decal;
        encode_downsample(
            &mut record,
            taps.uv,
            taps.kernel,
            taps.texel,
            edges.as_ref(),
            decal,
        );
        let bindings = Bindings::Textured(self.tiled_texture_bind(input, taps.tile_mode));
        Shading::of(Frag::Downsample, record, bindings)
    }

    /// `blur_pass_shading` is one direction of the Gaussian over `input`,
    /// a texture `size` texels big, its kernel beside the record.
    pub fn blur_pass_shading(
        &mut self,
        pass: &BlurPass,
        input: &wgpu::TextureView,
        size: [u32; 2],
    ) -> Shading {
        let mut record = UniformRecord::tinted([1.0; 4]);
        let texel = size.map(|size| 1.0 / size as f32);
        record.set(slot::GEOM, [texel[0], texel[1], 0.0, 0.0]);
        record.set(
            slot::MISC,
            [
                pass.sample_count() as f32,
                f32::from(pass.divide_by_alpha),
                0.0,
                0.0,
            ],
        );
        let bindings = Bindings::Blurred {
            texture: self.texture_bind(input),
            kernel: self.alloc_kernel(&pass.kernel_uniform()),
        };
        Shading::of(Frag::Blur, record, bindings)
    }

    /// `textured` binds one texture with the sampler that clamps at its
    /// edge.
    fn textured(&self, view: &wgpu::TextureView) -> Bindings {
        Bindings::Textured(self.texture_bind(view))
    }
}

/// `set_destination_read` writes an advanced blend's mode and the size of
/// the target whose copy the fragment reads at its framebuffer position.
fn set_destination_read(record: &mut UniformRecord, mode: AdvancedBlend, size: [u32; 2]) {
    record.set(
        slot::MISC,
        [mode.id() as f32, 0.0, size[0] as f32, size[1] as f32],
    );
}

/// `tinted` is what multiplies the fragment family's output. Solid = the
/// color itself; image/gradient sources use paint ALPHA only (Skia's
/// drawImage semantics). `extra` folds in an elided group's alpha.
pub(super) fn tinted(paint: &Paint, extra: f32) -> [f32; 4] {
    if paint.shader.is_none() {
        scaled_premul(paint.color, extra)
    } else {
        alpha_tint(paint.color.a * extra)
    }
}

pub(super) fn scaled_premul(color: Color, alpha: f32) -> [f32; 4] {
    let [r, g, b, a] = color.premultiplied();
    [r * alpha, g * alpha, b * alpha, a * alpha]
}

pub(super) fn alpha_tint(a: f32) -> [f32; 4] {
    [a, a, a, a]
}

/// `full_rect_uv` maps a full texture stretched across `rect` into uv.
fn full_rect_uv(rect: &Rect) -> [f32; 4] {
    let sx = 1.0 / rect.width;
    let sy = 1.0 / rect.height;
    [sx, sy, -rect.x * sx, -rect.y * sy]
}

/// `PaintSource` is where a paint's colour comes from: the one place a
/// shader is told apart, so what follows handles each kind alone.
enum PaintSource<'a> {
    Solid,
    Pattern {
        image: &'a Image,
        sampling: Sampling,
        local: &'a Matrix,
    },
    Gradient(Gradient<'a>),
}

impl<'a> PaintSource<'a> {
    fn of(paint: &'a Paint) -> Self {
        let Some(shader) = &paint.shader else {
            return PaintSource::Solid;
        };
        let (kind, stops, local) = match shader {
            Shader::Image {
                image,
                sampling,
                local,
            } => {
                return PaintSource::Pattern {
                    image,
                    sampling: *sampling,
                    local,
                }
            }
            Shader::Linear {
                start,
                end,
                stops,
                spread,
                local,
            } => (
                GradientKind::Linear {
                    start: *start,
                    end: *end,
                    spread: *spread,
                },
                stops,
                local,
            ),
            Shader::Radial {
                center,
                radius,
                focus,
                stops,
                spread,
                local,
            } => (
                GradientKind::Radial {
                    center: *center,
                    radius: *radius,
                    focus: *focus,
                    spread: *spread,
                },
                stops,
                local,
            ),
            Shader::Sweep {
                center,
                start_angle,
                stops,
                local,
            } => (
                GradientKind::Sweep {
                    center: *center,
                    start_angle: *start_angle,
                },
                stops,
                local,
            ),
        };
        PaintSource::Gradient(Gradient { kind, stops, local })
    }
}

/// `Gradient` is a gradient shader: its geometry, its stops and its local
/// matrix.
struct Gradient<'a> {
    kind: GradientKind,
    stops: &'a [GradientStop],
    local: &'a Matrix,
}

/// `GradientKind` is a gradient's geometry, and how it spreads past 0..1.
enum GradientKind {
    Linear {
        start: Point,
        end: Point,
        spread: SpreadMode,
    },
    Radial {
        center: Point,
        radius: f32,
        focus: Option<FocalCircle>,
        spread: SpreadMode,
    },
    /// A sweep is periodic by nature.
    Sweep { center: Point, start_angle: f32 },
}

impl Gradient<'_> {
    /// `frag` is the fragment of a gradient whose stops fit the record.
    fn frag(&self) -> Frag {
        match self.kind {
            GradientKind::Linear { .. } => Frag::Linear,
            GradientKind::Radial { .. } => Frag::Radial,
            GradientKind::Sweep { .. } => Frag::Sweep,
        }
    }

    /// `ramp_frag` is the fragment of a gradient baked into a ramp.
    fn ramp_frag(&self) -> Frag {
        match self.kind {
            GradientKind::Linear { .. } => Frag::LinearRamp,
            GradientKind::Radial { .. } => Frag::RadialRamp,
            GradientKind::Sweep { .. } => Frag::SweepRamp,
        }
    }
}

/// `is_opaque_paint` reports whether `paint`'s colour covers every pixel it
/// touches with full alpha.
fn is_opaque_paint(paint: &Paint) -> bool {
    paint.mask_blur.is_none()
        && paint.effective_image_filter().is_none()
        && paint.color.a >= 1.0
        && paint.shader.as_ref().is_none_or(shader_opaque)
}

fn shader_opaque(shader: &Shader) -> bool {
    // A two-point conical gradient with a real start circle does not cover
    // the plane: outside its cone nothing is painted at all, so opaque
    // promotion would turn those pixels into replaced black. A gradient that
    // can leave gaps never qualifies, however opaque its stops are.
    if let Shader::Radial {
        center,
        focus: Some(circle),
        ..
    } = shader
    {
        if circle.radius > 0.0 || circle.center != *center {
            return false;
        }
    }
    let stops = match shader {
        Shader::Linear { stops, .. }
        | Shader::Radial { stops, .. }
        | Shader::Sweep { stops, .. } => stops,
        // A pattern's alpha lives in texels nobody has read at plan time.
        Shader::Image { .. } => return false,
    };
    stops.iter().all(|stop| stop.color.a >= 1.0)
}

/// `EncodedColorFilter` names which fragment variant consumes the payload
/// [`encode_color_filter`] wrote.
#[derive(Clone, Copy)]
enum EncodedColorFilter {
    Matrix,
    Blend,
    Gamma,
}

impl EncodedColorFilter {
    /// `pass_fragment` runs the filter over a whole texture in a filter pass.
    fn pass_fragment(self) -> Frag {
        match self {
            EncodedColorFilter::Matrix => Frag::ColorMatrix,
            EncodedColorFilter::Blend => Frag::ColorBlend,
            EncodedColorFilter::Gamma => Frag::ColorGamma,
        }
    }

    /// `image_fragment` runs the filter on each sample of an image draw.
    fn image_fragment(self) -> Frag {
        match self {
            EncodedColorFilter::Matrix => Frag::ImageMatrix,
            EncodedColorFilter::Blend => Frag::ImageBlend,
            EncodedColorFilter::Gamma => Frag::ImageGamma,
        }
    }

    /// `glyph_fragment` runs the filter on each pixel of a colour glyph.
    fn glyph_fragment(self) -> Frag {
        match self {
            EncodedColorFilter::Matrix => Frag::GlyphColorMatrix,
            EncodedColorFilter::Blend => Frag::GlyphColorBlend,
            EncodedColorFilter::Gamma => Frag::GlyphColorGamma,
        }
    }
}

/// `encode_color_filter` writes Impeller's mat4-plus-translation-vector
/// layout, its constant premultiplied blend source, or the direction of the
/// sRGB gamma curve, and `input_alpha`: what the texel is multiplied by
/// before the filter (Impeller's `input_alpha`, which absorbs a layer's
/// opacity). Draw and filter-pass shaders share this ABI.
fn encode_color_filter(
    record: &mut UniformRecord,
    filter: ColorFilter,
    input_alpha: f32,
) -> EncodedColorFilter {
    match filter {
        ColorFilter::Matrix(matrix) => {
            for row in 0..4 {
                let start = row * 5;
                record.set(
                    slot::COLOR_MATRIX + row,
                    [
                        matrix[start],
                        matrix[start + 1],
                        matrix[start + 2],
                        matrix[start + 3],
                    ],
                );
            }
            record.set(
                slot::COLOR_MATRIX + 4,
                [matrix[4], matrix[9], matrix[14], matrix[19]],
            );
            record.set(slot::MISC, [0.0, 0.0, 0.0, input_alpha]);
            EncodedColorFilter::Matrix
        }
        ColorFilter::Blend(color, mode) => {
            record.set(slot::COLOR_MATRIX, color.premultiplied());
            let id = Blend::of(mode).filter_id() as f32;
            record.set(slot::MISC, [id, 0.0, 0.0, input_alpha]);
            EncodedColorFilter::Blend
        }
        ColorFilter::LinearToSrgbGamma => encode_gamma(record, gamma::LINEAR_TO_SRGB, input_alpha),
        ColorFilter::SrgbToLinearGamma => encode_gamma(record, gamma::SRGB_TO_LINEAR, input_alpha),
    }
}

fn encode_gamma(
    record: &mut UniformRecord,
    direction: f32,
    input_alpha: f32,
) -> EncodedColorFilter {
    record.set(slot::MISC, [direction, 0.0, 0.0, input_alpha]);
    EncodedColorFilter::Gamma
}

/// `encode_merge` writes the two-input fragments' payload (`fs_drop_shadow`,
/// `fs_mask_combine`): each input's uv mapping and the merge's switch, and
/// returns the fragment.
fn encode_merge(
    record: &mut UniformRecord,
    first_uv: [f32; 4],
    second_uv: [f32; 4],
    merge: Merge,
) -> Frag {
    let (frag, switch) = match merge {
        Merge::DropShadow => (Frag::DropShadow, 0.0),
        Merge::BlurStyle(style) => (Frag::MaskCombine, blur_style_id(style)),
    };
    record.set(slot::GEOM, first_uv);
    record.set(slot::SECOND_UV, second_uv);
    record.set(slot::MISC, [switch, 0.0, 0.0, 0.0]);
    frag
}

/// `DownsampleKernel` is the downsample fragment's taps: at odd texel
/// offsets out to `edge`, each weighted `ratio`.
#[derive(Clone, Copy, Debug, PartialEq)]
pub(super) struct DownsampleKernel {
    pub edge: f32,
    pub ratio: f32,
}

impl DownsampleKernel {
    /// `ONE_TAP` is one bilinear tap per pixel: a plain resample.
    pub const ONE_TAP: Self = Self {
        edge: 0.0,
        ratio: 1.0,
    };
}

/// `encode_downsample` writes the downsample fragment's payload: `uv` maps
/// the pass's own pixels into the input's uv as `uv = p · [x, y] + [z, w]`,
/// `kernel` is its taps, one input texel (`texel`, in uv) apart, `bounds`
/// the edge lines a bounded blur tests its taps against, and `decal` cuts
/// off taps past the input's edge.
fn encode_downsample(
    record: &mut UniformRecord,
    uv: [f32; 4],
    kernel: DownsampleKernel,
    texel: [f32; 2],
    bounds: Option<&[[f32; 4]; 4]>,
    decal: bool,
) {
    record.set(slot::GEOM, uv);
    record.set(slot::MISC, [kernel.edge, kernel.ratio, texel[0], texel[1]]);
    if let Some(lines) = bounds {
        for (index, line) in lines.iter().enumerate() {
            record.set(slot::DOWNSAMPLE_EDGES + index, *line);
        }
    }
    record.set(
        slot::DOWNSAMPLE_MODES,
        [f32::from(bounds.is_some()), f32::from(decal), 0.0, 0.0],
    );
}

/// `fill_gradient_payload` writes gradient geometry + stops into the
/// payload. A focal radial's fx/fy ride the two spare floats (geom.w /
/// misc.w); focus == center encodes "classic". The INVERSE of the shader's
/// local matrix lands in the local slots — fragments map draw space into
/// gradient space with it (identity for plain gradients; a non-invertible
/// matrix degenerates to a constant ramp sample, never UB). `ramp_texels` =
/// Some(N) when the stops ride a baked texture: the count lane carries N
/// for the fragment's half-texel mapping, and the uniform stop arrays stay
/// untouched (the texture IS the ramp).
fn fill_gradient_payload(
    record: &mut UniformRecord,
    gradient: &Gradient,
    ramp_texels: Option<u32>,
) {
    let (geom, angle, misc_w, spread) = match gradient.kind {
        GradientKind::Linear { start, end, spread } => {
            ([start.x, start.y, end.x, end.y], 0.0, 0.0, spread)
        }
        GradientKind::Radial {
            center,
            radius,
            focus,
            spread,
        } => {
            let f = focus.map_or(center, |circle| circle.center);
            ([center.x, center.y, radius, f.x], 0.0, f.y, spread)
        }
        GradientKind::Sweep {
            center,
            start_angle,
        } => (
            [center.x, center.y, 0.0, 0.0],
            start_angle,
            0.0,
            SpreadMode::Pad,
        ),
    };
    record.set(slot::GEOM, geom);
    let mut inverse = gradient
        .local
        .invert()
        .unwrap_or(Matrix::from_affine(0.0, 0.0, 0.0, 0.0, 0.0, 0.0));

    // A two-point conical gradient is solved in a space where the focal
    // point sits at the origin and the end circle is the unit circle. That
    // mapping is constant per draw, so it folds into the inverse local
    // matrix here and the fragment only runs the per-pixel half.
    let setup = match gradient.kind {
        GradientKind::Radial {
            center,
            radius,
            focus,
            ..
        } => ConicalSetup::solve(center, radius, focus),
        _ => ConicalSetup::UNUSED,
    };
    if let Some(focal_map) = setup.focal_map {
        inverse = focal_map.then(&inverse);
    }
    record.set(slot::CONICAL, setup.constants);
    record.set(slot::CONICAL_FLAGS, setup.flags);

    // Gradient locals are affine by construction — the 2D block is exact.
    let [a, b, c, d, tx, ty] = inverse.to_affine();
    record.set(slot::LOCAL, [a, b, c, d]);
    record.set(slot::LOCAL + 1, [tx, ty, 0.0, 0.0]);

    let count = gradient.stops.len().min(MAX_GRADIENT_STOPS);
    let count_lane = match ramp_texels {
        Some(texels) => texels as f32,
        None => count as f32,
    };
    record.set(slot::MISC, [count_lane, angle, spread_id(spread), misc_w]);
    if ramp_texels.is_some() {
        return;
    }
    let mut offsets = [0.0f32; MAX_GRADIENT_STOPS];
    for (i, stop) in gradient.stops.iter().take(count).enumerate() {
        offsets[i] = stop.offset;
        record.set(slot::COLORS + i, stop.color.components());
    }
    record.set(
        slot::OFFSETS,
        [offsets[0], offsets[1], offsets[2], offsets[3]],
    );
    record.set(
        slot::OFFSETS + 1,
        [offsets[4], offsets[5], offsets[6], offsets[7]],
    );
}

/// `fill_pattern_payload` writes a pattern's mapping: `local⁻¹` into
/// pattern pixels, then the reciprocal image size to reach uv. Tiling and
/// filtering ride the sampler.
fn fill_pattern_payload(
    record: &mut UniformRecord,
    image: &Image,
    sampling: Sampling,
    local: &Matrix,
) {
    let size = image.size();
    record.set(
        slot::GEOM,
        [1.0 / size[0] as f32, 1.0 / size[1] as f32, 0.0, 0.0],
    );
    record.set(slot::DECAL, decal_flags(sampling));
    let inverse = local
        .invert()
        .unwrap_or(Matrix::from_affine(0.0, 0.0, 0.0, 0.0, 0.0, 0.0));
    let [a, b, c, d, tx, ty] = inverse.to_affine();
    record.set(slot::LOCAL, [a, b, c, d]);
    record.set(slot::LOCAL + 1, [tx, ty, 0.0, 0.0]);
}

/// `image_uv` is uv = local × scale + offset, mapping `dst` (local px) onto
/// `src` (texture px, normalized) — out-of-range uv is the sampler's
/// business.
fn image_uv(image: &Image, src: &Rect, dst: &Rect) -> [f32; 4] {
    let (tw, th) = (image.width(), image.height());
    let sx = src.width / (dst.width * tw);
    let sy = src.height / (dst.height * th);
    [sx, sy, src.x / tw - dst.x * sx, src.y / th - dst.y * sy]
}

/// `decal_flags` is the per-axis decal switch: 1 where the fragment must
/// cut off outside the image, 0 where the sampler's own address mode
/// already produces the right pixels. Every image-sampling fragment reads
/// these from the decal slot, so a pattern and a direct `drawImage` honour
/// `TileMode::Decal` identically.
fn decal_flags(sampling: Sampling) -> [f32; 4] {
    [
        f32::from(sampling.tile_x == TileMode::Decal),
        f32::from(sampling.tile_y == TileMode::Decal),
        0.0,
        0.0,
    ]
}

/// `ConicalSetup` is which formula the fragment runs for a radial gradient,
/// plus the constants it needs. Skia's two-point conical algorithm
/// (skia.org/docs/dev/design/conical) splits into cases by where the focal
/// point lands; the choice and all of its precomputation are per-draw, so
/// they happen here rather than per fragment the way Impeller does it.
struct ConicalSetup {
    /// `(kind, local_r1, f, d_radius_sign)` — see `radial_t` in the shader.
    constants: [f32; 4],
    /// `(is_swapped, is_focal_on_circle, is_well_behaved, unused)`.
    flags: [f32; 4],
    /// Gradient space → focal space, when the general case needs it.
    focal_map: Option<Matrix>,
}

impl ConicalSetup {
    const UNUSED: Self = Self {
        constants: [conical::CONCENTRIC, 0.0, 0.0, 0.0],
        flags: [0.0; 4],
        focal_map: None,
    };

    /// `solve` runs Skia's `SkConicalGradient` decomposition, once per draw.
    fn solve(center: Point, radius: f32, focus: Option<FocalCircle>) -> Self {
        // Two different epsilons on purpose, following Impeller: case
        // SELECTION uses the looser `kEhCloseEnough` (conical_gradient_contents
        // .cc), because a separation just above the tight one produces a
        // near-singular 1/length map and fp32 noise with it. The tight
        // 1/4096 stays for the in-shader constants (gradient.glsl).
        const CASE_EPSILON: f32 = 1.0e-3;
        const NEARLY_ZERO: f32 = 1.0 / (1 << 12) as f32;
        let start = focus.unwrap_or(FocalCircle::point(center));
        let separation = (center.x - start.center.x).hypot(center.y - start.center.y);

        // Concentric circles need no focal machinery: t is just how far the
        // point sits between the two radii.
        if separation < CASE_EPSILON {
            if (radius - start.radius).abs() < CASE_EPSILON {
                return Self {
                    constants: [conical::EMPTY, 0.0, 0.0, 0.0],
                    ..Self::UNUSED
                };
            }
            return Self {
                constants: [conical::CONCENTRIC, start.radius, radius, 0.0],
                flags: [0.0; 4],
                focal_map: None,
            };
        }

        // Equal radii have no focal point at all — the circles sweep a strip
        // between their common tangents, and `focal` below would divide by
        // zero. Skia and Impeller both carve this out as its own case.
        if (radius - start.radius).abs() < CASE_EPSILON {
            let radius_in_unit_space = start.radius / separation;
            return Self {
                constants: [
                    conical::STRIP,
                    radius_in_unit_space * radius_in_unit_space,
                    0.0,
                    0.0,
                ],
                flags: [0.0; 4],
                focal_map: Some(map_to_unit_x(start.center, center)),
            };
        }

        // Steps 1-2: the focal parameter, and the swap that keeps it finite
        // when the two radii are equal.
        let (mut first, mut second) = (start.center, center);
        let mut focal = start.radius / (start.radius - radius);
        let is_swapped = (focal - 1.0).abs() < NEARLY_ZERO;
        if is_swapped {
            std::mem::swap(&mut first, &mut second);
            focal = 0.0f32;
        }

        // Steps 3-4: map [focal centre, end centre] onto [(0,0), (1,0)], then
        // scale so the end circle becomes the unit circle.
        let focal_center = Point::new(
            first.x * (1.0 - focal) + second.x * focal,
            first.y * (1.0 - focal) + second.y * focal,
        );
        let radius_in_unit_space = (radius - start.radius).abs() / separation;
        let is_focal_on_circle = (radius_in_unit_space - 1.0).abs() < NEARLY_ZERO;
        let span = (1.0 - focal).abs();
        let (scale_x, scale_y) = if is_focal_on_circle {
            (span * 0.5, span * 0.5)
        } else {
            let squared = radius_in_unit_space * radius_in_unit_space;
            (
                span * radius_in_unit_space / (squared - 1.0),
                span / (squared - 1.0).abs().sqrt(),
            )
        };

        let is_well_behaved = !is_focal_on_circle && radius_in_unit_space > 1.0;
        Self {
            constants: [
                conical::GENERAL,
                radius_in_unit_space,
                focal,
                (1.0 - focal).signum(),
            ],
            flags: [
                is_swapped as u32 as f32,
                is_focal_on_circle as u32 as f32,
                is_well_behaved as u32 as f32,
                0.0,
            ],
            focal_map: Some(scale_after(
                map_to_unit_x(focal_center, second),
                scale_x,
                scale_y,
            )),
        }
    }
}

/// `map_to_unit_x` maps `[from, to]` onto `[(0, 0), (1, 0)]`.
fn map_to_unit_x(from: Point, to: Point) -> Matrix {
    let (dx, dy) = (to.x - from.x, to.y - from.y);
    let length = dx.hypot(dy);
    let (ux, uy) = (dx / length, dy / length);
    Matrix::from_affine(
        ux / length,
        -uy / length,
        uy / length,
        ux / length,
        -(ux * from.x + uy * from.y) / length,
        (uy * from.x - ux * from.y) / length,
    )
}

/// `scale_after` is `scale(x, y) ∘ matrix` for an affine 2D matrix.
fn scale_after(matrix: Matrix, x: f32, y: f32) -> Matrix {
    let [a, b, c, d, tx, ty] = matrix.to_affine();
    Matrix::from_affine(a * x, b * y, c * x, d * y, tx * x, ty * y)
}
