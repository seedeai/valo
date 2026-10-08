//! Impeller's Gaussian blur (`GaussianBlurFilterContents`), ported for every
//! blur the planner runs: image filters, backdrops, drop shadows and mask
//! blurs. σ becomes a kernel the way Impeller computes it (`ScaleSigma`, the
//! radius per σ, `CalculateBlurInfo`, `CalculateScale`), and each blur is the
//! same three passes: a downsample that also cuts out the region (the whole
//! input with a gutter for the halo, read past its edge as the tile mode
//! says, or — for an input placed where the blur's entity puts it that holds
//! the coverage hint — the hint aligned to the downsample's divisor), then
//! the vertical pass, then the horizontal one, both sampling Impeller's
//! merged kernel two texels per bilinear fetch.
//!
//! The kernel reaches Impeller's blur radius, √3·(σ − ½)
//! (`GaussianBlurFilterContents::CalculateBlurRadius`, `Sigma`'s conversion
//! to `Radius` in `geometry/sigma.cc` with `kKernelRadiusPerSigma`), not the
//! 3σ a Skia or CSS blur reaches, so a halo ends shorter than theirs: at
//! σ = 3 it ends 4.3 pixels out. A blur's padding and coverage take the same
//! radius.
//!
//! The functions here are transcriptions; a change to one changes pixels.
//! The unit tests below are Impeller's own from
//! `gaussian_blur_filter_contents_unittests.cc`.

use valo_geometry::{Matrix, Point, Rect};

use valo_dl::TileMode;

use super::filter_passes::FilterPasses;
use super::shading::DownsampleKernel;
use super::snapshot::{Placement, Snapshot};

/// `MAX_SIGMA` is Impeller's `kMaxSigma`: the largest σ a blur takes, in
/// device pixels.
const MAX_SIGMA: f32 = 500.0;

/// `EH_CLOSE_ENOUGH` is Impeller's `kEhCloseEnough`: a σ below it does not
/// blur.
const EH_CLOSE_ENOUGH: f32 = 1e-3;

/// `KERNEL_RADIUS_PER_SIGMA` is Impeller's `kKernelRadiusPerSigma`.
const KERNEL_RADIUS_PER_SIGMA: f32 = 1.732_050_8;

/// `MAX_KERNEL_SAMPLES` is Impeller's `kGaussianBlurMaxKernelSize`: the
/// merged samples a blur pass reads, the size of its kernel uniform.
pub(crate) const MAX_KERNEL_SAMPLES: usize = 50;

/// `MAX_UNMERGED_SAMPLES` is Impeller's `KernelSamples::kMaxKernelSize`.
const MAX_UNMERGED_SAMPLES: usize = MAX_KERNEL_SAMPLES * 2;

/// `scale_sigma` is Impeller's `ScaleSigma`: σ clamped to [`MAX_SIGMA`] and
/// shrunk the way Skia's blur shrinks it at large σ. It applies to the σ a
/// filter was given, before the transform scales it.
pub(super) fn scale_sigma(sigma: f32) -> f32 {
    let clamped = sigma.min(MAX_SIGMA);
    const A: f32 = 3.4e-6;
    const B: f32 = -3.4e-3;
    const C: f32 = 1.0;
    let scalar = C + B * clamped + A * clamped * clamped;
    clamped * scalar
}

/// `basis_of` is a mapping's 2×2 linear part `[a, b, c, d]`: what the
/// filters that only scale, rotate or shear need of it.
pub(super) fn basis_of(mapping: &Matrix) -> [f32; 4] {
    let [a, b, c, d, ..] = mapping.to_affine();
    [a, b, c, d]
}

/// `extract_scale` is Impeller's `ExtractScale`: the lengths of a
/// transform's two axes, its scale without rotation or skew.
pub(super) fn extract_scale(transform: &Matrix) -> [f32; 2] {
    let [a, b, c, d] = basis_of(transform);
    [a.hypot(b), c.hypot(d)]
}

/// `device_sigma` maps a local-space blur σ onto the device axes, as
/// Impeller's `GaussianBlurFilterContents` does it: transform σ as a VECTOR
/// by the effect transform's basis, then take the component-wise absolute
/// value. Collapsing the basis to its two axis LENGTHS instead would lose
/// the rotation — a quarter turn has unit-length axes, so an anisotropic σ
/// would pass through unswapped and blur along the wrong axis.
fn device_sigma(basis: [f32; 4], sigma_x: f32, sigma_y: f32) -> [f32; 2] {
    let [a, b, c, d] = basis;
    [
        (sigma_x * a + sigma_y * c).abs(),
        (sigma_x * b + sigma_y * d).abs(),
    ]
}

/// `skia_sigma` is the same mapping as Skia's, for the filters whose
/// reference is Skia rather than Impeller: each axis BASIS maps separately,
/// taking its length (`SkImageFilterTypes.cpp`'s `mapSize`). The two rules
/// disagree under a 45° rotation — drop shadow follows Skia because
/// `SkImageFilters::DropShadow` is what CSS `drop-shadow()` lowers to;
/// a plain image filter blur keeps Impeller's rule.
pub(super) fn skia_sigma(basis: [f32; 4], sigma_x: f32, sigma_y: f32) -> [f32; 2] {
    let [a, b, c, d] = basis;
    [sigma_x * a.hypot(b), sigma_y * c.hypot(d)]
}

/// `blur_source_coverage` is `GaussianBlurFilterContents::GetFilterSourceCoverage`:
/// `output_limit` grown by the blur's radius for each local σ, taken onto
/// the device axes by the effect transform's basis.
pub(super) fn blur_source_coverage(
    effect_transform: &Matrix,
    sigma: [f32; 2],
    output_limit: &Rect,
) -> Rect {
    let radius = sigma.map(|sigma| blur_radius(scale_sigma(sigma)));
    let radii = device_sigma(basis_of(effect_transform), radius[0], radius[1]);
    expanded(output_limit, radii)
}

/// `blur_radius` is Impeller's `CalculateBlurRadius`: half the kernel, in
/// the pixels σ is in.
pub(super) fn blur_radius(sigma: f32) -> f32 {
    if sigma > 0.5 {
        (sigma - 0.5) * KERNEL_RADIUS_PER_SIGMA
    } else {
        0.0
    }
}

/// `calculate_scale` is Impeller's `CalculateScale`: the downsample's scale
/// for σ. σ ≤ 4 runs at full resolution; past that the scale halves until
/// the effective σ is ~4, never below 1/16, and 1/16 becomes 1/8 while the
/// kernel at 1/8 stays within 41 taps.
fn calculate_scale(sigma: f32) -> f32 {
    if sigma <= 4.0 {
        return 1.0;
    }
    let raw_result = 4.0 / sigma;
    let exponent = raw_result.log2().round().max(-4.0);
    let rounded = exponent.exp2();
    if rounded < 0.125 {
        let rounded_plus = (exponent + 1.0).exp2();
        let kernel_size_plus = scale_blur_radius(blur_radius(sigma), rounded_plus) * 2 + 1;
        // Impeller's `kEighthDownsampleKernalWidthMax`.
        if kernel_size_plus <= 41 {
            return rounded_plus;
        }
    }
    rounded
}

/// `scale_blur_radius` is Impeller's `ScaleBlurRadius`.
fn scale_blur_radius(radius: f32, scalar: f32) -> i32 {
    (radius * scalar).round() as i32
}

/// `BlurInfo` is Impeller's `BlurInfo` for a blur already on the device
/// axes: σ, the kernel radius and the halo's padding, per axis.
#[derive(Clone, Copy, Debug, PartialEq)]
pub(super) struct BlurInfo {
    pub scaled_sigma: [f32; 2],
    blur_radius: [f32; 2],
    padding: [f32; 2],
    local_padding: [f32; 2],
}

impl BlurInfo {
    /// `calculate` is Impeller's `CalculateBlurInfo`: each σ through
    /// [`scale_sigma`], scaled by the entity's axis lengths (its source
    /// space; the rest of the entity is left to where the result is drawn),
    /// then taken onto the device axes by the effect transform's basis.
    ///
    /// A blur in a layer has an entity of scale one and the CTM as its
    /// effect transform. A blur in a draw's source space has the draw's
    /// transform as its entity and none as its effect transform, which is
    /// how Impeller runs a mask blur.
    pub fn calculate(entity: &Matrix, effect_transform: &Matrix, sigma: [f32; 2]) -> Self {
        let scalar = extract_scale(entity);
        let source = [0, 1].map(|axis| scalar[axis] * scale_sigma(sigma[axis]));
        let mut info = Self::of(device_sigma(
            basis_of(effect_transform),
            source[0],
            source[1],
        ));
        info.local_padding = [0, 1].map(|axis| (scalar[axis] * info.padding[axis]).abs());
        info
    }

    /// `of` takes σ already on the device axes and clamps it to
    /// [`MAX_SIGMA`], as `CalculateBlurInfo` does: for the filters whose σ
    /// rule is Skia's.
    pub fn of(scaled_sigma: [f32; 2]) -> Self {
        let scaled_sigma = scaled_sigma.map(|sigma| sigma.clamp(0.0, MAX_SIGMA));
        let blur_radius = scaled_sigma.map(blur_radius);
        let padding = blur_radius.map(f32::ceil);
        Self {
            scaled_sigma,
            blur_radius,
            padding,
            local_padding: padding,
        }
    }

    /// `in_texels` is this blur over an input whose texels map into the
    /// tree's space by `texel_to_tree`: σ, in the tree's pixels, divided by
    /// how many of them a texel spans on each axis. A departure from
    /// Impeller, which blurs a texel as if it were a pixel, so that an image
    /// drawn whole at twice its size blurs twice as far
    /// (`TextureContentsWithDestinationRectScaled`); so does a blur reading
    /// an earlier blur's downsampled output. Snapshots stay axis-aligned in
    /// the tree's space, so each axis divides by its own length.
    pub fn in_texels(&self, texel_to_tree: &Matrix) -> Self {
        let texel = extract_scale(texel_to_tree);
        Self::of([0, 1].map(|axis| self.scaled_sigma[axis] / texel[axis]))
    }

    /// `is_negligible` is Impeller's test for a blur that returns its input
    /// as it is: neither σ reaches [`EH_CLOSE_ENOUGH`].
    pub fn is_negligible(&self) -> bool {
        self.scaled_sigma[0] < EH_CLOSE_ENOUGH && self.scaled_sigma[1] < EH_CLOSE_ENOUGH
    }

    /// `radius` is how far the blur reaches on each side, per axis, in the
    /// pixels σ is in: Impeller's `blur_radius`.
    pub fn radius(&self) -> [f32; 2] {
        self.blur_radius
    }

    /// `local_padding` is Impeller's `local_padding`: the halo's padding
    /// (what the downsample's gutter holds) scaled by the entity's axis
    /// lengths, what the blur's coverage grows by.
    /// Impeller scales a padding already in source pixels once more, so for
    /// an entity that scales this is more than the blur reaches; it only
    /// bounds.
    pub fn local_padding(&self) -> [f32; 2] {
        self.local_padding
    }

    /// `downsample_scale` is `CalculateDownsamplePassArgs`'s `desired_scalar`:
    /// one scale for both axes, the smaller.
    fn downsample_scale(&self) -> f32 {
        calculate_scale(self.scaled_sigma[0]).min(calculate_scale(self.scaled_sigma[1]))
    }
}

/// `KernelSample` is Impeller's `KernelSample`: one tap, as an offset in uv
/// from the pixel being blurred, and its weight.
#[derive(Clone, Copy, Debug, PartialEq)]
struct KernelSample {
    uv_offset: [f32; 2],
    coefficient: f32,
}

/// `BlurParameters` is Impeller's `BlurParameters` for one blur pass.
struct BlurParameters {
    /// One texel along the pass's axis, in uv.
    blur_uv_offset: [f32; 2],
    blur_sigma: f32,
    blur_radius: i32,
    step_size: i32,
}

impl BlurParameters {
    /// `along` is the pass of `blur` along `axis` (0 is x, 1 is y) over the
    /// texture the downsample `args` describe: one of its texels along that
    /// axis, and σ and the radius at its resolution.
    fn along(axis: usize, blur: &BlurInfo, args: &DownsamplePassArgs) -> Self {
        let scalar = args.effective_scalar[axis];
        let mut blur_uv_offset = [0.0; 2];
        blur_uv_offset[axis] = 1.0 / args.subpass_size[axis] as f32;
        Self {
            blur_uv_offset,
            blur_sigma: blur.scaled_sigma[axis] * scalar,
            blur_radius: scale_blur_radius(blur.blur_radius[axis], scalar),
            step_size: 1,
        }
    }

    /// `is_negligible` is Impeller's test for a pass it skips.
    fn is_negligible(&self) -> bool {
        self.blur_sigma < EH_CLOSE_ENOUGH
    }
}

/// `generate_blur_info` is Impeller's `GenerateBlurInfo`: the Gaussian's
/// taps from `-radius` to `radius`, normalized to sum to 1. From a radius of
/// 16 the outermost tap on each side is dropped, and no more than
/// [`MAX_UNMERGED_SAMPLES`] are kept.
fn generate_blur_info(parameters: &BlurParameters) -> Vec<KernelSample> {
    let mut sample_count = ((2 * parameters.blur_radius) / parameters.step_size) + 1;
    let mut x_offset = 0;
    if parameters.blur_radius >= 16 {
        sample_count -= 2;
        x_offset = 1;
    }
    let sample_count = (sample_count as usize).min(MAX_UNMERGED_SAMPLES);
    let sigma = parameters.blur_sigma;
    let mut samples: Vec<KernelSample> = (0..sample_count)
        .map(|index| {
            let x = x_offset + (index as i32 * parameters.step_size) - parameters.blur_radius;
            let x = x as f32;
            KernelSample {
                uv_offset: parameters.blur_uv_offset.map(|offset| offset * x),
                coefficient: (-0.5 * (x * x) / (sigma * sigma)).exp()
                    / ((2.0 * std::f32::consts::PI).sqrt() * sigma),
            }
        })
        .collect();
    let tally: f32 = samples.iter().map(|sample| sample.coefficient).sum();
    for sample in &mut samples {
        sample.coefficient /= tally;
    }
    samples
}

/// `lerp_hack_kernel_samples` is Impeller's `LerpHackKernelSamples`: every
/// two neighbouring taps but the middle one become one bilinear fetch
/// between them, weighted by both, so a pass reads about half the texels.
fn lerp_hack_kernel_samples(samples: &[KernelSample]) -> Vec<KernelSample> {
    let sample_count = ((samples.len() - 1) / 2) + 1;
    let middle = sample_count / 2;
    let mut merged = Vec::with_capacity(sample_count);
    let mut j = 0;
    for index in 0..sample_count {
        if index == middle {
            merged.push(samples[j]);
            j += 1;
        } else {
            let (left, right) = (samples[j], samples[j + 1]);
            let coefficient = left.coefficient + right.coefficient;
            let uv_offset = [0, 1].map(|axis| {
                (left.uv_offset[axis] * left.coefficient
                    + right.uv_offset[axis] * right.coefficient)
                    / coefficient
            });
            merged.push(KernelSample {
                uv_offset,
                coefficient,
            });
            j += 2;
        }
    }
    merged
}

/// `InputKind` picks between `CalculateDownsamplePassArgs`'s two branches.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum InputKind {
    /// The region is cut from a larger picture, Impeller's coverage-hint
    /// branch: the cut is aligned to the downsample's divisor so that it
    /// does not shimmer as it moves, and what lies around it is read as it
    /// is, up to the texture's edge.
    Cut,
    /// The region is the whole input: a gutter as wide as the kernel
    /// surrounds it, for the halo, read the way the tile mode says.
    Whole,
}

/// `BlurPlan` is one blur of an input, worked out before any pass runs:
/// Impeller's `CalculateDownsamplePassArgs` for the input and its coverage
/// hint, the downsample's taps, each direction's kernel, and where the
/// result lies. It reads the input's [`Placement`] alone, so it needs no
/// GPU.
#[derive(Debug, PartialEq)]
pub(super) struct BlurPlan {
    args: DownsamplePassArgs,
    downsample: DownsampleTaps,
    /// The vertical pass; `None` when its σ is negligible.
    vertical: Option<BlurPass>,
    /// The horizontal pass; `None` when its σ is negligible, unless a
    /// bounded blur still divides by alpha in it.
    horizontal: Option<BlurPass>,
    /// Where the result's texels lie: the whole downsampled texture, gutter
    /// or aligned margin included, placed where the input's region was, as
    /// Impeller's output snapshot is.
    output: Placement,
}

/// `DownsampleTaps` is how a blur's downsample reads its input
/// (`MakeDownsampleSubpass`).
#[derive(Debug, PartialEq)]
pub(super) struct DownsampleTaps {
    pub kernel: DownsampleKernel,
    /// Maps the pass's own pixels into the input's uv.
    pub uv: [f32; 4],
    /// One input texel, in uv.
    pub texel: [f32; 2],
    /// A bounded blur's edges, in the input's uv.
    pub edges: Option<EdgeLines>,
    /// The sampler's address mode past the input's edge (Impeller's
    /// `SetTileMode`), a decal cut off in the shader.
    pub tile_mode: TileMode,
}

/// `BlurPass` is one direction of the Gaussian (`MakeBlurSubpass`).
#[derive(Debug, PartialEq)]
pub(super) struct BlurPass {
    samples: Vec<KernelSample>,
    /// A bounded blur's last pass divides by alpha.
    pub divide_by_alpha: bool,
}

/// `BlurBounds` is what a bounded blur reads within: a rectangle in local
/// coordinates and the transform from them into the tree's space.
pub(super) struct BlurBounds<'a> {
    pub rect: &'a Rect,
    pub local_to_tree: &'a Matrix,
}

impl BlurBounds<'_> {
    /// `texel_quad` is the rectangle's corners, clockwise on screen from the
    /// top-left, in `input`'s texels.
    fn texel_quad(&self, input: &Placement) -> [Point; 4] {
        let local_to_texels = input.texel_transform().then(self.local_to_tree);
        self.rect
            .corners()
            .map(|corner| local_to_texels.map_point(corner))
    }
}

impl BlurPlan {
    /// `new` plans `blur`, σ in the tree's pixels, over an input placed by
    /// `input`; `None` for a negligible blur, which leaves its input as it
    /// is. Over texels that are not the tree's pixels σ is divided by their
    /// size ([`BlurInfo::in_texels`]). A placed input that holds `hint`
    /// (the coverage hint grown by the blur's padding) is cut to it, as
    /// Impeller chooses between `CalculateDownsamplePassArgs`'s branches;
    /// any other is read whole, with a gutter, past its edge as `tile_mode`
    /// says. So a backdrop at the target's edge, whose hint reaches past the
    /// parent, blurs the whole parent. `bounds` makes it a bounded blur: the
    /// downsample reads only inside them and the last pass divides by alpha.
    pub fn new(
        blur: &BlurInfo,
        input: &Placement,
        hint: Option<&Rect>,
        tile_mode: TileMode,
        bounds: Option<BlurBounds<'_>>,
    ) -> Option<Self> {
        if blur.is_negligible() {
            return None;
        }
        let blur = blur.in_texels(&input.transform);
        let texels = input.size.map(|size| size as f32);
        let (region, kind) = match cut_region(&input.coverage(), hint.filter(|_| input.placed)) {
            Some(hint) => (input.texel_rect(hint), InputKind::Cut),
            None => (Rect::new(0.0, 0.0, texels[0], texels[1]), InputKind::Whole),
        };
        let args = downsample_pass_args(&blur, texels, &region, kind);
        let edges =
            bounds.map(|bounds| EdgeLines::of(&bounds.texel_quad(input), texel_to_uv(texels)));
        let bounded = edges.is_some();
        Some(Self {
            downsample: DownsampleTaps::of(&args, texels, edges, tile_mode),
            vertical: BlurPass::along(1, &blur, &args, false),
            horizontal: BlurPass::along(0, &blur, &args, bounded),
            output: Placement {
                size: args.subpass_size,
                transform: blurred_transform(&input.transform, &args),
                placed: false,
            },
            args,
        })
    }
}

impl DownsampleTaps {
    /// `of` is `MakeDownsampleSubpass`'s reading of `args.uvs` of an input
    /// `texels` big into a target `args.subpass_size` big: one bilinear tap
    /// per pixel or the 4-, 16- or 64-tap kernel, a bounded blur's taps
    /// tested against `edges`.
    fn of(
        args: &DownsamplePassArgs,
        texels: [f32; 2],
        edges: Option<EdgeLines>,
        tile_mode: TileMode,
    ) -> Self {
        let work = args.subpass_size.map(|size| size as f32);
        let [u0, v0, u1, v1] = args.uvs;
        Self {
            kernel: downsample_kernel(args.effective_scalar[0], edges.is_some()),
            uv: [(u1 - u0) / work[0], (v1 - v0) / work[1], u0, v0],
            texel: texels.map(|texels| 1.0 / texels),
            edges,
            tile_mode,
        }
    }
}

impl BlurPass {
    /// `sample_count` is how many merged samples the pass reads.
    pub fn sample_count(&self) -> usize {
        self.samples.len()
    }

    /// `kernel_uniform` packs the pass's samples into its kernel uniform,
    /// Impeller's `KernelSamples` block: `[u, v, coefficient, 0]` each.
    pub fn kernel_uniform(&self) -> [[f32; 4]; MAX_KERNEL_SAMPLES] {
        let mut uniform = [[0.0; 4]; MAX_KERNEL_SAMPLES];
        for (slot, sample) in uniform.iter_mut().zip(&self.samples) {
            *slot = [
                sample.uv_offset[0],
                sample.uv_offset[1],
                sample.coefficient,
                0.0,
            ];
        }
        uniform
    }

    /// `along` is the pass of `blur` along `axis` (0 is x, 1 is y) over the
    /// downsample `args` describe; `None` when its σ is negligible, as
    /// Impeller skips it — except a bounded blur's last pass, which still
    /// runs for its division by alpha (dart:ui documents the division for
    /// every bounded blur; Impeller loses it when σx ≈ 0).
    fn along(
        axis: usize,
        blur: &BlurInfo,
        args: &DownsamplePassArgs,
        divide_by_alpha: bool,
    ) -> Option<Self> {
        let parameters = BlurParameters::along(axis, blur, args);
        let samples = if !parameters.is_negligible() {
            lerp_hack_kernel_samples(&generate_blur_info(&parameters))
        } else if divide_by_alpha {
            vec![KernelSample {
                uv_offset: [0.0, 0.0],
                coefficient: 1.0,
            }]
        } else {
            return None;
        };
        Some(Self {
            samples,
            divide_by_alpha,
        })
    }
}

/// `texel_to_uv` maps the texels of a texture `texels` big to its uv, as
/// `uv = p · [x, y] + [z, w]`.
fn texel_to_uv(texels: [f32; 2]) -> [f32; 4] {
    [1.0 / texels[0], 1.0 / texels[1], 0.0, 0.0]
}

/// `cut_region` is the containment test of `CalculateDownsamplePassArgs`:
/// the input hint a placed input covering `coverage` is cut to, or `None`
/// for a gutter.
fn cut_region<'h>(coverage: &Rect, input_hint: Option<&'h Rect>) -> Option<&'h Rect> {
    input_hint.filter(|hint| contains(coverage, hint))
}

/// `contains` is Impeller's `Rect::Contains` for a rect: `inner` lies within
/// `outer`, edges included.
fn contains(outer: &Rect, inner: &Rect) -> bool {
    !inner.is_empty()
        && inner.x >= outer.x
        && inner.y >= outer.y
        && inner.right() <= outer.right()
        && inner.bottom() <= outer.bottom()
}

/// `DownsamplePassArgs` is Impeller's `DownsamplePassArgs`, in the input
/// texture's texels.
#[derive(Clone, Copy, Debug, PartialEq)]
struct DownsamplePassArgs {
    /// The downsample target's size.
    subpass_size: [u32; 2],
    /// What the pass reads: `[u0, v0, u1, v1]` of the input.
    uvs: [f32; 4],
    effective_scalar: [f32; 2],
    /// What the downsampled texture covers, in the input's texels.
    covers: Rect,
}

/// `downsample_pass_args` is Impeller's `CalculateDownsamplePassArgs`.
/// `texels` is the input texture's extent and `region` the part of it the
/// blur reads, in its texels.
fn downsample_pass_args(
    blur: &BlurInfo,
    texels: [f32; 2],
    region: &Rect,
    kind: InputKind,
) -> DownsamplePassArgs {
    let scalar = blur.downsample_scale();
    match kind {
        InputKind::Cut => aligned_cut_args(scalar, texels, region),
        InputKind::Whole => gutter_args(scalar, blur.padding, texels, region),
    }
}

/// `blurred_transform` is the transform of a blur's output snapshot:
/// `input_transform` (the input's texels into replay coordinates), then
/// where the downsample's target lies in the input, at its own resolution
/// (Impeller's `pass_args.transform · S(1 / effective_scalar)`).
fn blurred_transform(input_transform: &Matrix, args: &DownsamplePassArgs) -> Matrix {
    input_transform
        .then(&Matrix::translation(args.covers.x, args.covers.y))
        .then(&Matrix::scale(
            1.0 / args.effective_scalar[0],
            1.0 / args.effective_scalar[1],
        ))
}

/// `aligned_cut_args` is `CalculateDownsamplePassArgs`'s coverage-hint
/// branch: the region's origin floored and its size ceiled to multiples of
/// the downsample's divisor, read straight out of the texture.
fn aligned_cut_args(scalar: f32, texels: [f32; 2], region: &Rect) -> DownsamplePassArgs {
    let divisor = (1.0 / scalar).round();
    let left = floor_to_divisible(region.x, divisor);
    let top = floor_to_divisible(region.y, divisor);
    let aligned = Rect::new(
        left,
        top,
        ceil_to_divisible(region.right() - left, divisor),
        ceil_to_divisible(region.bottom() - top, divisor),
    );
    let source_size = [aligned.width.trunc(), aligned.height.trunc()];
    let subpass_size = source_size.map(|size| (size * scalar) as u32);
    let uvs = [
        (aligned.x / texels[0]).clamp(0.0, 1.0),
        (aligned.y / texels[1]).clamp(0.0, 1.0),
        (aligned.right() / texels[0]).clamp(0.0, 1.0),
        (aligned.bottom() / texels[1]).clamp(0.0, 1.0),
    ];
    DownsamplePassArgs {
        subpass_size,
        uvs,
        effective_scalar: [0, 1].map(|axis| subpass_size[axis] as f32 / source_size[axis]),
        covers: Rect::new(aligned.x, aligned.y, source_size[0], source_size[1]),
    }
}

/// `gutter_args` is `CalculateDownsamplePassArgs`'s other branch: the region
/// grown by the kernel's padding, rounded so the grown size divides by the
/// downsample's divisor; the gutter reads as the tile mode says.
fn gutter_args(
    scalar: f32,
    padding: [f32; 2],
    texels: [f32; 2],
    region: &Rect,
) -> DownsamplePassArgs {
    let source_rect = *region;
    let padded = expanded(&source_rect, padding);
    let subpass_size = [
        (padded.width * scalar).ceil() as u32,
        (padded.height * scalar).ceil() as u32,
    ];
    let divisible_size = [
        ceil_to_divisible(padded.width, 1.0 / scalar),
        ceil_to_divisible(padded.height, 1.0 / scalar),
    ];
    // Padding only grows to divisible where there is padding already: more
    // added to a hard blur edge shows.
    let divisible_padding = [
        divisible_gutter(padding[0], divisible_size[0], padded.width),
        divisible_gutter(padding[1], divisible_size[1], padded.height),
    ];
    let covers = expanded(&source_rect, divisible_padding);
    DownsamplePassArgs {
        subpass_size,
        uvs: [
            covers.x / texels[0],
            covers.y / texels[1],
            covers.right() / texels[0],
            covers.bottom() / texels[1],
        ],
        effective_scalar: [
            subpass_size[0] as f32 / covers.width,
            subpass_size[1] as f32 / covers.height,
        ],
        covers,
    }
}

/// `expanded` is Impeller's `Rect::Expand` by a per-axis amount.
pub(super) fn expanded(rect: &Rect, amount: [f32; 2]) -> Rect {
    Rect::new(
        rect.x - amount[0],
        rect.y - amount[1],
        rect.width + 2.0 * amount[0],
        rect.height + 2.0 * amount[1],
    )
}

fn divisible_gutter(padding: f32, divisible: f32, padded: f32) -> f32 {
    if padding > 0.0 {
        padding + (divisible - padded) / 2.0
    } else {
        0.0
    }
}

/// `ceil_to_divisible` is Impeller's `CeilToDivisible`.
fn ceil_to_divisible(value: f32, divisor: f32) -> f32 {
    if divisor == 0.0 {
        return value;
    }
    let remainder = value % divisor;
    if remainder != 0.0 {
        value + (divisor - remainder)
    } else {
        value
    }
}

/// `floor_to_divisible` is Impeller's `FloorToDivisible`.
fn floor_to_divisible(value: f32, divisor: f32) -> f32 {
    if divisor == 0.0 {
        return value;
    }
    let remainder = value % divisor;
    if remainder != 0.0 {
        value - remainder
    } else {
        value
    }
}

/// `downsample_kernel` is `MakeDownsampleSubpass`'s kernel: taps at odd
/// texel offsets out to `edge`, each weighted `ratio`. A pass that shrinks
/// by half or less reads one bilinear tap, as Impeller's plain texture copy
/// does — unless it is bounded, which always tests its taps.
fn downsample_kernel(effective_scale: f32, bounded: bool) -> DownsampleKernel {
    if !bounded && effective_scale >= 0.5 {
        return DownsampleKernel::ONE_TAP;
    }
    let (edge, ratio) = if effective_scale <= 0.0625 {
        (7.0, 1.0 / 64.0)
    } else if effective_scale <= 0.125 {
        (3.0, 1.0 / 16.0)
    } else {
        (1.0, 0.25)
    };
    DownsampleKernel { edge, ratio }
}

/// `EdgeLines` are a quad's four edges as line equations `[a, b, c, 0]` in
/// a texture's uv space, each `a·u + b·v + c ≥ 0` on the quad's side —
/// Impeller's `PrecomputeQuadLineParameters`.
#[derive(Clone, Copy, Debug, PartialEq)]
pub(super) struct EdgeLines(pub [[f32; 4]; 4]);

impl EdgeLines {
    /// `of` takes `quad`'s corners, clockwise on screen from the top-left
    /// and in the input's texels, into uv by `texel_to_uv`. Impeller's
    /// equations face inward only
    /// for a quad in that winding; a mirroring transform reverses it, so the
    /// signs flip with it.
    pub fn of(quad: &[Point; 4], texel_to_uv: [f32; 4]) -> Self {
        let [x, y, z, w] = texel_to_uv;
        let corners = quad.map(|corner| Point::new(corner.x * x + z, corner.y * y + w));
        let inward = if winding_area(&corners) < 0.0 {
            -1.0
        } else {
            1.0
        };
        let edge = |from: Point, to: Point| {
            let a = from.y - to.y;
            let b = to.x - from.x;
            let c = -(a * from.x + b * from.y);
            [a * inward, b * inward, c * inward, 0.0]
        };
        Self([
            edge(corners[0], corners[1]),
            edge(corners[1], corners[2]),
            edge(corners[2], corners[3]),
            edge(corners[3], corners[0]),
        ])
    }
}

/// `winding_area` is twice the signed area of a polygon: positive when its
/// corners run clockwise on a y-down screen.
fn winding_area(corners: &[Point; 4]) -> f32 {
    (0..4)
        .map(|index| {
            let (from, to) = (corners[index], corners[(index + 1) % 4]);
            from.x * to.y - to.x * from.y
        })
        .sum()
}

impl FilterPasses<'_, '_> {
    /// `push_blur` is Impeller's `GaussianBlurFilterContents::RenderFilter`
    /// for one planned blur of `input`, a texture placed as the plan was
    /// given: the downsample, the vertical pass, the horizontal pass.
    pub fn push_blur(&mut self, plan: &BlurPlan, input: &wgpu::TextureView) -> Snapshot {
        let size = plan.args.subpass_size;
        let downsampled = self.push_downsample(&plan.downsample, size, input);
        let vertical = plan
            .vertical
            .as_ref()
            .map(|pass| self.push_blur_pass(pass, size, &downsampled, None));
        // Ping-pong back into the downsample's target when the vertical
        // pass made one of its own, as Impeller does.
        let horizontal = plan.horizontal.as_ref().map(|pass| match &vertical {
            Some(vertical) => self.push_blur_pass(pass, size, vertical, Some(downsampled.clone())),
            None => self.push_blur_pass(pass, size, &downsampled, None),
        });
        Snapshot {
            view: horizontal.or(vertical).unwrap_or(downsampled),
            placement: plan.output,
        }
    }

    /// `push_downsample` is Impeller's `MakeDownsampleSubpass`: one pass that
    /// reads `input` as `taps` say into a target `size` texels big.
    fn push_downsample(
        &mut self,
        taps: &DownsampleTaps,
        size: [u32; 2],
        input: &wgpu::TextureView,
    ) -> wgpu::TextureView {
        let target = self.pool.take_filter(size, self.format);
        let shading = self.emit.downsample_shading(taps, input);
        self.push_pass(
            target.view.clone(),
            self.format,
            &whole(size),
            size,
            shading,
        );
        target.view
    }

    /// `push_blur_pass` is Impeller's `MakeBlurSubpass`: one direction of the
    /// Gaussian over `input`, a texture `size` texels big, into
    /// `destination` or a new target, which it returns.
    fn push_blur_pass(
        &mut self,
        pass: &BlurPass,
        size: [u32; 2],
        input: &wgpu::TextureView,
        destination: Option<wgpu::TextureView>,
    ) -> wgpu::TextureView {
        let target = destination.unwrap_or_else(|| self.pool.take_filter(size, self.format).view);
        let shading = self.emit.blur_pass_shading(pass, input, size);
        self.push_pass(target.clone(), self.format, &whole(size), size, shading);
        target
    }
}

/// `whole` is a texture `size` texels big, as a rect of its own pixels.
fn whole(size: [u32; 2]) -> Rect {
    Rect::new(0.0, 0.0, size[0] as f32, size[1] as f32)
}

/// `sigma_for_blur_radius` is the unit tests' `CalculateSigmaForBlurRadius`
/// with its `LowerBoundNewtonianMethod`: the local σ whose radius under
/// `effect_transform`, the larger of the two axes', is `radius`, found by
/// Newton's method in the same precisions — the function in `f32`, the
/// iteration in `f64`.
#[cfg(test)]
pub(super) fn sigma_for_blur_radius(radius: f32, effect_transform: &Matrix) -> f32 {
    let basis = basis_of(effect_transform);
    let radius_of = |sigma: f32| {
        let scaled = device_sigma(basis, scale_sigma(sigma), scale_sigma(sigma));
        blur_radius(scaled[0]).max(blur_radius(scaled[1]))
    };
    let target = f64::from(radius);
    let (delta, mut x) = (1e-6f64, 2.0f64);
    // A do-while: the step is taken before the test, so the answer is one
    // step past the first one inside the tolerance.
    loop {
        let fx = f64::from(radius_of(x as f32)) - target;
        let derivative =
            (f64::from(radius_of((x + delta) as f32)) - f64::from(radius_of(x as f32))) / delta;
        x -= fx / derivative;
        if fx.abs() <= 0.001 && fx >= 0.0 {
            return x as f32;
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A 100×80 picture of the tree's pixels at (20, 10), placed or not.
    fn picture(placed: bool) -> Placement {
        Placement {
            size: [100, 80],
            transform: Matrix::translation(20.0, 10.0),
            placed,
        }
    }

    fn sigma(sigma: f32) -> BlurInfo {
        BlurInfo::calculate(&Matrix::IDENTITY, &Matrix::IDENTITY, [sigma; 2])
    }

    fn contains_rect(outer: &Rect, inner: &Rect) -> bool {
        inner.x >= outer.x
            && inner.y >= outer.y
            && inner.right() <= outer.right()
            && inner.bottom() <= outer.bottom()
    }

    #[test]
    fn a_negligible_blur_plans_nothing() {
        let plan = BlurPlan::new(&sigma(0.0), &picture(true), None, TileMode::Decal, None);
        assert_eq!(plan, None);
    }

    /// A placed picture that holds the hint is cut to it, and only that is
    /// read; one that is not placed is read whole, whatever the hint.
    #[test]
    fn a_blur_cuts_a_placed_picture_to_its_hint() {
        let hint = Rect::new(40.0, 30.0, 20.0, 20.0);
        let cut = BlurPlan::new(
            &sigma(2.0),
            &picture(true),
            Some(&hint),
            TileMode::Decal,
            None,
        )
        .expect("a blur");
        assert!(contains_rect(&cut.output.coverage(), &hint));
        assert!(cut.output.coverage().width < 30.0, "{:?}", cut.output);
        let whole = BlurPlan::new(
            &sigma(2.0),
            &picture(false),
            Some(&hint),
            TileMode::Decal,
            None,
        )
        .expect("a blur");
        assert!(contains_rect(
            &whole.output.coverage(),
            &picture(false).coverage()
        ));
    }

    /// A hint past the picture's edge is not held: the picture is read
    /// whole, its edge read past as the tile mode says.
    #[test]
    fn a_hint_past_the_pictures_edge_reads_it_whole() {
        let hint = Rect::new(0.0, 0.0, 50.0, 50.0);
        let plan = BlurPlan::new(
            &sigma(2.0),
            &picture(true),
            Some(&hint),
            TileMode::Mirror,
            None,
        )
        .expect("a blur");
        assert!(contains_rect(
            &plan.output.coverage(),
            &picture(true).coverage()
        ));
        assert_eq!(plan.downsample.tile_mode, TileMode::Mirror);
    }

    /// A bounded blur divides by alpha in its last pass, which it keeps even
    /// when that direction's σ is negligible; an unbounded one skips it.
    #[test]
    fn a_bounded_blur_keeps_its_last_pass_to_divide_by_alpha() {
        let vertical_only = BlurInfo::calculate(&Matrix::IDENTITY, &Matrix::IDENTITY, [0.0, 4.0]);
        let unbounded = BlurPlan::new(&vertical_only, &picture(false), None, TileMode::Decal, None)
            .expect("a blur");
        assert!(unbounded.vertical.is_some());
        assert_eq!(unbounded.horizontal, None);
        let rect = Rect::new(30.0, 20.0, 50.0, 40.0);
        let bounds = BlurBounds {
            rect: &rect,
            local_to_tree: &Matrix::IDENTITY,
        };
        let bounded = BlurPlan::new(
            &vertical_only,
            &picture(false),
            None,
            TileMode::Decal,
            Some(bounds),
        )
        .expect("a blur");
        let last = bounded.horizontal.expect("the division by alpha");
        assert!(last.divide_by_alpha);
        assert_eq!(last.samples.len(), 1);
        assert!(bounded.downsample.edges.is_some());
    }

    /// σ is in the tree's pixels: over texels two of them wide it blurs
    /// half as many texels, and the result is placed at the texels' scale.
    #[test]
    fn a_blur_over_wide_texels_blurs_fewer_of_them() {
        let wide = Placement {
            transform: Matrix::translation(20.0, 10.0).then(&Matrix::scale(2.0, 2.0)),
            ..picture(false)
        };
        let over_wide = BlurPlan::new(&BlurInfo::of([8.0; 2]), &wide, None, TileMode::Decal, None)
            .expect("a blur");
        let over_pixels = BlurPlan::new(
            &BlurInfo::of([4.0; 2]),
            &picture(false),
            None,
            TileMode::Decal,
            None,
        )
        .expect("a blur");
        assert_eq!(over_wide.args, over_pixels.args);
        assert_eq!(over_wide.vertical, over_pixels.vertical);
        assert_eq!(
            over_wide.output.transform.to_affine()[0],
            2.0 * over_pixels.output.transform.to_affine()[0]
        );
    }

    #[test]
    fn calculate_sigma_values() {
        assert_eq!(calculate_scale(1.0), 1.0);
        assert_eq!(calculate_scale(2.0), 1.0);
        assert_eq!(calculate_scale(3.0), 1.0);
        assert_eq!(calculate_scale(4.0), 1.0);
        assert_eq!(calculate_scale(16.0), 0.25);
        // Hang on to 1/8 as long as possible.
        assert_eq!(calculate_scale(95.0), 0.125);
        assert_eq!(calculate_scale(96.0), 0.0625);
        // Downsample clamped to 1/16th.
        assert_eq!(calculate_scale(1024.0), 0.0625);
    }

    #[test]
    fn calculate_sigma_for_blur_radius() {
        let sigma = 1.0;
        let radius = blur_radius(scale_sigma(sigma));
        assert!((sigma - sigma_for_blur_radius(radius, &Matrix::IDENTITY)).abs() < 0.01);
    }

    /// Impeller's `CoverageWithSigma`: a σ whose radius is 1 pads by 1.
    #[test]
    fn coverage_with_sigma() {
        let sigma = sigma_for_blur_radius(1.0, &Matrix::IDENTITY);
        let blur = BlurInfo::of([scale_sigma(sigma); 2]);
        assert_eq!(blur.padding, [1.0, 1.0]);
    }

    /// Impeller's `RenderCoverageMatchesGetCoverage` family, for the
    /// placement alone: the blurred snapshot of a 100×100 texture covers what
    /// the blur's `GetCoverage` says it does.
    fn rendered_coverage(texture_origin: Point, sigma: f32) -> (Rect, Rect) {
        let texture = Rect::new(texture_origin.x, texture_origin.y, 100.0, 100.0);
        let blur = BlurInfo::calculate(&Matrix::IDENTITY, &Matrix::IDENTITY, [sigma; 2]);
        let args = downsample_pass_args(
            &blur,
            [100.0, 100.0],
            &Rect::new(0.0, 0.0, 100.0, 100.0),
            InputKind::Whole,
        );
        let transform = blurred_transform(
            &Matrix::translation(texture_origin.x, texture_origin.y),
            &args,
        );
        let [width, height] = args.subpass_size.map(|size| size as f32);
        let rendered = transform.map_rect(&Rect::new(0.0, 0.0, width, height));
        let tree = super::super::filter_tree::FilterInput::Source.with_image_filter(
            Some(&valo_dl::ImageFilter::blur(sigma, sigma)),
            TileMode::Decal,
        );
        (rendered, tree.coverage(&texture, &Matrix::IDENTITY))
    }

    fn assert_rect_near(actual: Rect, expected: Rect) {
        let near = |a: f32, b: f32| (a - b).abs() < 1e-3;
        assert!(
            near(actual.x, expected.x)
                && near(actual.y, expected.y)
                && near(actual.right(), expected.right())
                && near(actual.bottom(), expected.bottom()),
            "{actual:?} != {expected:?}"
        );
    }

    #[test]
    fn render_coverage_matches_get_coverage() {
        let sigma = sigma_for_blur_radius(1.0, &Matrix::IDENTITY);
        let (rendered, coverage) = rendered_coverage(Point::new(0.0, 0.0), sigma);
        assert_rect_near(coverage, Rect::from_ltrb(-1.0, -1.0, 101.0, 101.0));
        assert_rect_near(rendered, Rect::from_ltrb(-1.0, -1.0, 101.0, 101.0));
    }

    #[test]
    fn render_coverage_matches_get_coverage_translate() {
        let sigma = sigma_for_blur_radius(1.0, &Matrix::IDENTITY);
        let (rendered, coverage) = rendered_coverage(Point::new(100.0, 200.0), sigma);
        assert_rect_near(coverage, Rect::from_ltrb(99.0, 199.0, 201.0, 301.0));
        assert_rect_near(rendered, Rect::from_ltrb(99.0, 199.0, 201.0, 301.0));
    }

    /// Not one of Impeller's: a blur that downsamples places its texture
    /// over the input and its halo all the same.
    #[test]
    fn a_downsampled_blur_covers_its_halo() {
        let (rendered, coverage) = rendered_coverage(Point::new(10.0, 20.0), 20.0);
        assert!(rendered.x <= coverage.x && rendered.right() >= coverage.right());
        assert!(rendered.y <= coverage.y && rendered.bottom() >= coverage.bottom());
        assert!(
            (rendered.width - coverage.width).abs() < 8.0,
            "{rendered:?} vs {coverage:?}"
        );
    }

    /// `CalculateDownsamplePassArgs` cuts an input placed where the entity
    /// puts it when it holds the input hint; anything else takes the
    /// gutter: a hint past the input's edge, or no hint at all (which is how
    /// an input the entity did not place is handed over).
    #[test]
    fn the_downsample_cuts_a_placed_input_that_holds_its_hint() {
        let parent = Rect::new(0.0, 0.0, 100.0, 80.0);
        let inside = Rect::new(10.0, 10.0, 40.0, 40.0);
        let past_the_top = Rect::new(10.0, -6.0, 40.0, 40.0);
        assert_eq!(cut_region(&parent, Some(&inside)), Some(&inside));
        assert_eq!(cut_region(&parent, Some(&past_the_top)), None);
        assert_eq!(cut_region(&parent, None), None);
    }

    #[test]
    fn coefficients() {
        let samples = generate_blur_info(&BlurParameters {
            blur_uv_offset: [1.0, 0.0],
            blur_sigma: 1.0,
            blur_radius: 5,
            step_size: 1,
        });
        assert_eq!(samples.len(), 11);
        let tally: f32 = samples.iter().map(|sample| sample.coefficient).sum();
        assert!((tally - 1.0).abs() < 1e-6);
        for index in 0..4 {
            assert_eq!(samples[index].coefficient, samples[10 - index].coefficient);
            assert!(samples[index + 1].coefficient > samples[index].coefficient);
        }
    }

    fn sample(x: f32, coefficient: f32) -> KernelSample {
        KernelSample {
            uv_offset: [x, 0.0],
            coefficient,
        }
    }

    #[test]
    fn lerp_hack_kernel_samples_simple() {
        let samples = [
            sample(-2.0, 0.1),
            sample(-1.0, 0.2),
            sample(0.0, 0.4),
            sample(1.0, 0.2),
            sample(2.0, 0.1),
        ];
        let merged = lerp_hack_kernel_samples(&samples);
        assert_eq!(merged.len(), 3);
        assert!((merged[0].uv_offset[0] + 1.333_333_3).abs() < 1e-5);
        assert!((merged[0].coefficient - 0.3).abs() < 1e-6);
        assert_eq!(merged[1].uv_offset, [0.0, 0.0]);
        assert!((merged[1].coefficient - 0.4).abs() < 1e-6);
        assert!((merged[2].uv_offset[0] - 1.333_333).abs() < 1e-5);
        assert!((merged[2].coefficient - 0.3).abs() < 1e-6);

        let data = [0.25, 0.5, 0.5, 1.0, 0.2];
        let original: f32 = samples
            .iter()
            .zip(data)
            .map(|(sample, value)| sample.coefficient * value)
            .sum();
        let lerp = |x: f32, left: f32, right: f32| {
            let fract = x.fract().abs();
            if x < 0.0 {
                left * fract + right * (1.0 - fract)
            } else {
                left * (1.0 - fract) + right * fract
            }
        };
        let fast = lerp(merged[0].uv_offset[0], data[0], data[1]) * merged[0].coefficient
            + data[2] * merged[1].coefficient
            + lerp(merged[2].uv_offset[0], data[3], data[4]) * merged[2].coefficient;
        assert!((original - fast).abs() < 0.01);
    }

    #[test]
    fn lerp_hack_kernel_samples_complex() {
        let sigma = 10.0;
        let radius = blur_radius(sigma).ceil() as i32;
        let samples = generate_blur_info(&BlurParameters {
            blur_uv_offset: [1.0, 0.0],
            blur_sigma: sigma,
            blur_radius: radius,
            step_size: 1,
        });
        assert_eq!(samples.len(), 33);
        let merged = lerp_hack_kernel_samples(&samples);
        assert_eq!(merged.len(), 17);
        // A fixed pseudo-random row in place of Impeller's `rand()`.
        let data: Vec<f32> = (0..33u32)
            .map(|index| ((index.wrapping_mul(2_654_435_761) >> 8) % 256) as f32)
            .collect();
        let sampler = |x: f32| {
            let fract = x.fract().abs();
            if fract == 0.0 {
                data[(x as i32 + 16) as usize]
            } else {
                let (left, right) = (
                    data[(x.floor() as i32 + 16) as usize],
                    data[(x.ceil() as i32 + 16) as usize],
                );
                if x < 0.0 {
                    fract * left + (1.0 - fract) * right
                } else {
                    (1.0 - fract) * left + fract * right
                }
            }
        };
        let output: f32 = samples
            .iter()
            .map(|sample| sample.coefficient * sampler(sample.uv_offset[0]))
            .sum();
        let fast: f32 = merged
            .iter()
            .map(|sample| sample.coefficient * sampler(sample.uv_offset[0]))
            .sum();
        assert!((output - fast).abs() < 0.1, "{output} vs {fast}");
    }

    #[test]
    fn chop_huge_blurs() {
        let sigma = 30.5;
        let samples = generate_blur_info(&BlurParameters {
            blur_uv_offset: [1.0, 0.0],
            blur_sigma: sigma,
            blur_radius: blur_radius(sigma).ceil() as i32,
            step_size: 1,
        });
        assert!(lerp_hack_kernel_samples(&samples).len() <= MAX_KERNEL_SAMPLES);
    }

    #[test]
    fn edge_lines_hold_the_inside_of_a_rect_in_uv() {
        let quad = Rect::new(20.0, 30.0, 40.0, 60.0).corners();
        let edges = EdgeLines::of(&quad, [0.01, 0.01, 0.0, 0.0]);
        assert!(inside(&edges, Point::new(0.4, 0.6)));
        for outside in [(0.1, 0.6), (0.7, 0.6), (0.4, 0.2), (0.4, 0.95)] {
            assert!(
                !inside(&edges, Point::new(outside.0, outside.1)),
                "{outside:?}"
            );
        }
    }

    #[test]
    fn edge_lines_follow_a_rotated_quad() {
        let quarter_turn = valo_geometry::Matrix::rotation(std::f32::consts::FRAC_PI_2);
        let quad = Rect::new(0.0, 0.0, 40.0, 10.0)
            .corners()
            .map(|corner| quarter_turn.map_point(corner));
        let edges = EdgeLines::of(&quad, [1.0, 1.0, 0.0, 0.0]);
        // The quarter turn stands the wide rect up to the left of the origin.
        assert!(inside(&edges, Point::new(-5.0, 20.0)));
        assert!(!inside(&edges, Point::new(20.0, 5.0)));
    }

    /// A mirror reverses the corners' winding, which would turn Impeller's
    /// equations inside out and mask everything.
    #[test]
    fn edge_lines_face_inward_under_a_mirror() {
        let mirror = valo_geometry::Matrix::translation(100.0, 0.0)
            .then(&valo_geometry::Matrix::scale(-1.0, 1.0));
        let quad = Rect::new(20.0, 30.0, 40.0, 60.0)
            .corners()
            .map(|corner| mirror.map_point(corner));
        let edges = EdgeLines::of(&quad, [1.0, 1.0, 0.0, 0.0]);
        assert!(inside(&edges, Point::new(60.0, 50.0)));
        assert!(!inside(&edges, Point::new(30.0, 50.0)));
    }

    fn inside(edges: &EdgeLines, point: Point) -> bool {
        edges
            .0
            .iter()
            .all(|[a, b, c, _]| a * point.x + b * point.y + c >= 0.0)
    }

    fn assert_sigma(actual: [f32; 2], expected: [f32; 2]) {
        assert!(
            (actual[0] - expected[0]).abs() < 1e-4 && (actual[1] - expected[1]).abs() < 1e-4,
            "sigma {actual:?} != {expected:?}"
        );
    }

    /// Drop shadow follows `SkImageFilters::Blur`'s `SkSize` mapping, so a
    /// rotation must leave an isotropic σ isotropic. Impeller's vector rule
    /// turns the same input into a directional smear.
    #[test]
    fn skia_sigma_keeps_a_rotated_blur_round() {
        let basis = basis_of(&Matrix::rotation(std::f32::consts::FRAC_PI_4));
        assert_sigma(skia_sigma(basis, 10.0, 10.0), [10.0, 10.0]);
        assert_sigma(device_sigma(basis, 10.0, 10.0), [0.0, 14.142136]);
    }

    #[test]
    fn skia_sigma_still_scales_each_axis() {
        let basis = basis_of(&Matrix::scale(2.0, 3.0));
        assert_sigma(skia_sigma(basis, 4.0, 5.0), [8.0, 15.0]);
    }

    #[test]
    fn device_sigma_scales_each_axis() {
        let basis = basis_of(&Matrix::scale(2.0, 3.0));
        assert_sigma(device_sigma(basis, 4.0, 5.0), [8.0, 15.0]);
    }

    /// A quarter turn maps local x onto device y. Reducing the basis to its
    /// axis LENGTHS would give [1, 1] here and leave σ unswapped — the bug
    /// this function exists to prevent.
    #[test]
    fn device_sigma_swaps_axes_under_a_quarter_turn() {
        let basis = basis_of(&Matrix::rotation(std::f32::consts::FRAC_PI_2));
        assert_sigma(device_sigma(basis, 12.0, 3.0), [3.0, 12.0]);
    }

    /// Impeller takes the component-wise absolute value, so a half turn is
    /// indistinguishable from no rotation at all.
    #[test]
    fn device_sigma_is_sign_agnostic() {
        let basis = basis_of(&Matrix::rotation(std::f32::consts::PI));
        assert_sigma(device_sigma(basis, 7.0, 2.0), [7.0, 2.0]);
    }

    /// Rotation composed with non-uniform scale: each device axis takes a
    /// contribution from BOTH local sigmas, which is the case an axis-length
    /// reduction cannot express at any angle.
    #[test]
    fn device_sigma_mixes_axes_under_rotation_and_scale() {
        let matrix = Matrix::scale(2.0, 3.0).then(&Matrix::rotation(std::f32::consts::FRAC_PI_4));
        let [a, b, c, d, ..] = matrix.to_affine();
        let expected = [(6.0 * a + 4.0 * c).abs(), (6.0 * b + 4.0 * d).abs()];
        assert_sigma(device_sigma(basis_of(&matrix), 6.0, 4.0), expected);
        // Both axes genuinely mix — a degenerate basis would pass vacuously.
        assert!(expected[0] > 1.0 && expected[1] > 1.0);
    }

    /// A mirror has a negative determinant; `.Abs()` makes it equivalent to
    /// its unmirrored twin.
    #[test]
    fn device_sigma_ignores_a_mirror() {
        let mirrored = basis_of(&Matrix::scale(-2.0, 3.0));
        assert_sigma(device_sigma(mirrored, 4.0, 5.0), [8.0, 15.0]);
    }

    /// A blur whose entity scales takes σ through each axis's length, after
    /// Impeller's `ScaleSigma`: the entity's scale, not the effect
    /// transform's vector.
    #[test]
    fn calculate_scales_sigma_by_the_entitys_axes() {
        let scaled = scale_sigma(10.0);
        let blur = BlurInfo::calculate(&Matrix::scale(2.0, 3.0), &Matrix::IDENTITY, [10.0; 2]);
        assert_sigma(blur.scaled_sigma, [2.0 * scaled, 3.0 * scaled]);
    }

    /// The entity's rotation is left to where the result goes: in a draw's
    /// source space a turned blur runs along the draw's own axes.
    #[test]
    fn calculate_leaves_the_entitys_rotation_out() {
        let entity = Matrix::rotation(std::f32::consts::FRAC_PI_4).then(&Matrix::scale(2.0, 2.0));
        let blur = BlurInfo::calculate(&entity, &Matrix::IDENTITY, [10.0, 0.0]);
        assert_sigma(blur.scaled_sigma, [2.0 * scale_sigma(10.0), 0.0]);
    }

    /// The effect transform takes σ as a vector, as `device_sigma` does.
    #[test]
    fn calculate_takes_sigma_through_the_effect_transform_as_a_vector() {
        let effect = Matrix::rotation(std::f32::consts::FRAC_PI_2);
        let blur = BlurInfo::calculate(&Matrix::IDENTITY, &effect, [12.0, 3.0]);
        assert_sigma(blur.scaled_sigma, [scale_sigma(3.0), scale_sigma(12.0)]);
    }

    /// Impeller's `local_padding` scales the padding by the entity once more.
    #[test]
    fn local_padding_is_the_padding_scaled_by_the_entity() {
        let blur = BlurInfo::calculate(&Matrix::scale(2.0, 2.0), &Matrix::IDENTITY, [4.0; 2]);
        assert_eq!(
            blur.local_padding(),
            blur.padding.map(|padding| 2.0 * padding)
        );
    }

    /// Not one of Impeller's: over texels two pixels wide and half a pixel
    /// tall, σ halves along x and doubles along y; over texels a pixel wide
    /// it is the blur as it was.
    #[test]
    fn a_blur_over_scaled_texels_keeps_its_reach_in_pixels() {
        let blur = BlurInfo::of([8.0, 8.0]);
        let texels = Matrix::translation(30.0, 40.0).then(&Matrix::scale(2.0, 0.5));
        assert_eq!(blur.in_texels(&texels).scaled_sigma, [4.0, 16.0]);
        assert_eq!(blur.in_texels(&Matrix::translation(30.0, 40.0)), blur);
    }
}
