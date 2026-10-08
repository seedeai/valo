//! Render pipelines: what a draw's fragment and role need of one, the blend
//! a pipeline runs, and the grow-only cache of compiled variants.

use rustc_hash::FxHashMap;

use valo_dl::BlendMode;

/// `SAMPLE_COUNT` is the MSAA sample count used by content pipelines.
///
/// Surfaces render into a 4-sample scratch and resolve at pass end. Filter
/// passes use 1 sample.
pub const SAMPLE_COUNT: u32 = 4;
/// `DEPTH_FORMAT` is the combined depth/stencil format used by content pipelines.
///
/// One buffer serves depth clips and stencil-then-cover.
pub const DEPTH_FORMAT: wgpu::TextureFormat = wgpu::TextureFormat::Depth24PlusStencil8;

/// `MESH_VERTEX_FLOATS` is how many floats a stroke's, fan's or strip's
/// vertex carries: its local position.
pub(crate) const MESH_VERTEX_FLOATS: usize = 2;

/// `GLYPH_VERTEX_FLOATS` is how many floats a glyph quad's vertex carries:
/// its position, then its atlas uv.
pub(crate) const GLYPH_VERTEX_FLOATS: usize = 4;

/// `Frag` selects the fragment shader that colors a covered pixel.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum Frag {
    Solid,
    Image,
    /// Direct-image color filters run after texture sampling.
    ImageMatrix,
    ImageBlend,
    ImageGamma,
    Linear,
    Radial,
    Sweep,
    /// Advanced blend, solid src × dst snapshot (group1 = snapshot).
    BlendSolid,
    /// Advanced blend, texture src (layer / desugared draw) × dst snapshot
    /// (group1 = snapshot + src texture).
    BlendTexture,
    /// Closed-form blurred solid (r)rect — soft coverage, zero filter passes.
    RRectBlur,
    /// One direction of a separable gaussian, Impeller's merged kernel in
    /// the host buffer at group 2 (filter passes only; blur layout).
    Blur,
    /// A blur's downsample: one bilinear tap or Impeller's 4-, 16- or
    /// 64-tap kernel, a gutter read as transparent, a bounded blur's taps
    /// tested against its bounds (filter passes only).
    Downsample,
    /// Blur style combine: blurred layer × sharp layer → one texture
    /// (filter passes only; blend layout: 0 = blurred, 2 = sharp).
    MaskCombine,
    /// Drop-shadow combine: the sharp layer over its offset blurred shadow
    /// (filter passes only; blend layout: 0 = shadow, 2 = sharp).
    DropShadow,
    /// Mask layer composite: texture → coverage in alpha
    /// (luminance or alpha per payload flag), drawn with DstIn.
    MaskComposite,
    /// Gradients past 8 stops sampling a baked 1D ramp texture.
    LinearRamp,
    RadialRamp,
    SweepRamp,
    /// Colour filters over a layer's texture (filter passes only): a 4×5
    /// matrix, a constant colour blended as the source, or the sRGB gamma
    /// curve in either direction.
    ColorMatrix,
    ColorBlend,
    ColorGamma,
    /// An image tiled across the shape, sampled through the paint's own
    /// local matrix — Canvas2D's pattern.
    Pattern,
    /// Glyph quads over an R8 coverage page × tint (the pixel-aligned
    /// bitmap tier).
    GlyphMask,
    /// Glyph quads over an R8 distance field thresholded at 0.5 (the
    /// transformed tier).
    GlyphSdf,
    /// Glyph quads over RGBA colour glyphs (emoji) × an alpha-only tint.
    GlyphColor,
    /// Colour glyphs through a colour filter on each pixel, as an image's
    /// samples go through theirs: a 4×5 matrix, a constant colour blended as
    /// the source, or the sRGB gamma curve.
    GlyphColorMatrix,
    GlyphColorBlend,
    GlyphColorGamma,
}

/// `BindLayout` is which bind groups a fragment reads beside its uniforms.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum BindLayout {
    /// The uniforms alone.
    Plain,
    /// One texture and its sampler at group 1.
    Textured,
    /// A destination copy, a sampler and a source texture at group 1.
    Blend,
    /// A texture at group 1, a blur kernel at group 2.
    Blur,
}

impl Frag {
    /// `bind_layout` is which bind groups this fragment reads.
    fn bind_layout(self) -> BindLayout {
        match self {
            Frag::Solid | Frag::Linear | Frag::Radial | Frag::Sweep | Frag::RRectBlur => {
                BindLayout::Plain
            }
            Frag::BlendTexture | Frag::MaskCombine | Frag::DropShadow => BindLayout::Blend,
            Frag::Blur => BindLayout::Blur,
            Frag::Image
            | Frag::ImageMatrix
            | Frag::ImageBlend
            | Frag::ImageGamma
            | Frag::BlendSolid
            | Frag::Downsample
            | Frag::MaskComposite
            | Frag::LinearRamp
            | Frag::RadialRamp
            | Frag::SweepRamp
            | Frag::ColorMatrix
            | Frag::ColorBlend
            | Frag::ColorGamma
            | Frag::Pattern
            | Frag::GlyphMask
            | Frag::GlyphSdf
            | Frag::GlyphColor
            | Frag::GlyphColorMatrix
            | Frag::GlyphColorBlend
            | Frag::GlyphColorGamma => BindLayout::Textured,
        }
    }

    fn entry_point(self) -> &'static str {
        match self {
            Frag::Solid => "fs_solid",
            Frag::Image => "fs_image",
            Frag::ImageMatrix => "fs_image_matrix",
            Frag::ImageBlend => "fs_image_blend",
            Frag::ImageGamma => "fs_image_gamma",
            Frag::Linear => "fs_linear",
            Frag::Radial => "fs_radial",
            Frag::Sweep => "fs_sweep",
            Frag::BlendSolid => "fs_blend_solid",
            Frag::BlendTexture => "fs_blend_texture",
            Frag::RRectBlur => "fs_rrect_blur",
            Frag::Blur => "fs_blur",
            Frag::Downsample => "fs_downsample",
            Frag::MaskCombine => "fs_mask_combine",
            Frag::DropShadow => "fs_drop_shadow",
            Frag::MaskComposite => "fs_mask_composite",
            Frag::LinearRamp => "fs_linear_ramp",
            Frag::RadialRamp => "fs_radial_ramp",
            Frag::SweepRamp => "fs_sweep_ramp",
            Frag::ColorMatrix => "fs_color_matrix",
            Frag::ColorBlend => "fs_color_blend",
            Frag::ColorGamma => "fs_color_gamma",
            Frag::Pattern => "fs_pattern",
            Frag::GlyphMask => "fs_text",
            Frag::GlyphSdf => "fs_text_sdf",
            Frag::GlyphColor => "fs_text_color",
            Frag::GlyphColorMatrix => "fs_text_color_matrix",
            Frag::GlyphColorBlend => "fs_text_color_blend",
            Frag::GlyphColorGamma => "fs_text_color_gamma",
        }
    }
}

/// `PipelineKind` selects the vertex source and the color, depth, and stencil role.
///
/// Any fragment family composes with either color role: a gradient can fill a
/// path cover quad as readily as a rectangle.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum PipelineKind {
    /// Plain colored quad draw (rects, images, gradients).
    Draw(Frag),
    /// StC pass 2: quad gated on stencil != 0, resetting it to 0.
    Cover(Frag),
    /// `Draw`, but provably opaque: writes DEPTH so earlier
    /// (lower-z) fragments early-z-cull under it; blending off (replace).
    OpaqueDraw(Frag),
    /// `Cover`, opaque: stencil-gated quad that also writes depth.
    OpaqueCover(Frag),
    /// StC pass 1: path fan into the STENCIL buffer only (no color, no depth).
    StencilFan { even_odd: bool },
    /// A stroke's triangle strip into the STENCIL buffer only, every
    /// triangle counting up where it lands: the stencil half of a clip to a
    /// stroke, whose triangles overlap.
    StencilStrip,
    /// Depth-clip ceiling: z=expiry written outside (Intersect) or
    /// inside (Difference) the stenciled shape; no color.
    ClipCover { difference: bool },
    /// Bare color work between the frame's passes (gaussian blur chains):
    /// 1-sample, no depth/stencil, output replaces the target.
    Filter(Frag),
    /// Stroke geometry: a CPU triangle STRIP along the path;
    /// depth-tested like a draw, any fragment family composes.
    Strip(Frag),
    /// Atlas-masked glyph quads (pos + uv vertices), one of the glyph
    /// fragments.
    Glyphs(Frag),
}

impl PipelineKind {
    fn writes_color(self) -> bool {
        matches!(
            self,
            PipelineKind::Draw(_)
                | PipelineKind::Cover(_)
                | PipelineKind::OpaqueDraw(_)
                | PipelineKind::OpaqueCover(_)
                | PipelineKind::Filter(_)
                | PipelineKind::Strip(_)
                | PipelineKind::Glyphs(_)
        )
    }

    fn frag(self) -> Option<Frag> {
        match self {
            PipelineKind::Draw(f)
            | PipelineKind::Cover(f)
            | PipelineKind::OpaqueDraw(f)
            | PipelineKind::OpaqueCover(f)
            | PipelineKind::Filter(f)
            | PipelineKind::Strip(f)
            | PipelineKind::Glyphs(f) => Some(f),
            PipelineKind::StencilFan { .. }
            | PipelineKind::StencilStrip
            | PipelineKind::ClipCover { .. } => None,
        }
    }

    /// Output replaces dst — pipeline blending off: opaque draws (nothing
    /// shows through α=1), filter passes (fresh targets), and advanced
    /// blends (the shader already composited against the snapshot).
    fn replaces_dst(self) -> bool {
        matches!(
            self,
            PipelineKind::OpaqueDraw(_) | PipelineKind::OpaqueCover(_) | PipelineKind::Filter(_)
        ) || matches!(
            self.frag(),
            Some(Frag::BlendSolid) | Some(Frag::BlendTexture)
        )
    }

    fn fragment_entry(self) -> &'static str {
        self.frag().map_or("fs_solid", Frag::entry_point)
    }

    /// `sample_count` returns 1 for filter passes and [`SAMPLE_COUNT`] otherwise.
    pub fn sample_count(self) -> u32 {
        match self {
            PipelineKind::Filter(_) => 1,
            _ => SAMPLE_COUNT,
        }
    }

    fn vertex_entry(self) -> &'static str {
        match self {
            PipelineKind::StencilFan { .. }
            | PipelineKind::StencilStrip
            | PipelineKind::Strip(_) => "vs_mesh",
            PipelineKind::Glyphs(_) => "vs_text",
            _ => "vs_quad",
        }
    }

    /// `blends` reports whether this kind's colour goes through pipeline
    /// blending: it writes colour and does not replace the destination.
    fn blends(self) -> bool {
        self.writes_color() && !self.replaces_dst()
    }

    /// Blend only matters where color is written AND blended; normalizing
    /// the rest de-duplicates cache entries.
    fn normalized_blend(self, blend: PipelineBlend) -> PipelineBlend {
        if self.blends() {
            blend
        } else {
            PipelineBlend::SrcOver
        }
    }

    /// `bind_layout` is which group-1 (and group-2) bindings this kind's
    /// fragment reads.
    fn bind_layout(self) -> BindLayout {
        self.frag().map_or(BindLayout::Plain, Frag::bind_layout)
    }
}

/// `Blend` is how a draw's colour combines with what is beneath it,
/// decided once, while routing: by the blend unit, or in the fragment
/// against a copy of the destination.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum Blend {
    /// The blend unit runs it.
    Fixed(PipelineBlend),
    /// The fragment reads a copy of the destination and blends itself.
    ReadsDestination(AdvancedBlend),
}

impl Blend {
    /// `of` is how `mode` is run.
    pub fn of(mode: BlendMode) -> Self {
        use AdvancedBlend as A;
        use PipelineBlend as P;
        match mode {
            BlendMode::Clear => Blend::Fixed(P::Clear),
            BlendMode::Src => Blend::Fixed(P::Src),
            BlendMode::Dst => Blend::Fixed(P::Dst),
            BlendMode::SrcOver => Blend::Fixed(P::SrcOver),
            BlendMode::DstOver => Blend::Fixed(P::DstOver),
            BlendMode::SrcIn => Blend::Fixed(P::SrcIn),
            BlendMode::DstIn => Blend::Fixed(P::DstIn),
            BlendMode::SrcOut => Blend::Fixed(P::SrcOut),
            BlendMode::DstOut => Blend::Fixed(P::DstOut),
            BlendMode::SrcAtop => Blend::Fixed(P::SrcAtop),
            BlendMode::DstAtop => Blend::Fixed(P::DstAtop),
            BlendMode::Xor => Blend::Fixed(P::Xor),
            BlendMode::Plus => Blend::Fixed(P::Plus),
            BlendMode::Modulate => Blend::Fixed(P::Modulate),
            BlendMode::Screen => Blend::Fixed(P::Screen),
            BlendMode::Multiply => Blend::ReadsDestination(A::Multiply),
            BlendMode::Overlay => Blend::ReadsDestination(A::Overlay),
            BlendMode::Darken => Blend::ReadsDestination(A::Darken),
            BlendMode::Lighten => Blend::ReadsDestination(A::Lighten),
            BlendMode::ColorDodge => Blend::ReadsDestination(A::ColorDodge),
            BlendMode::ColorBurn => Blend::ReadsDestination(A::ColorBurn),
            BlendMode::HardLight => Blend::ReadsDestination(A::HardLight),
            BlendMode::SoftLight => Blend::ReadsDestination(A::SoftLight),
            BlendMode::Difference => Blend::ReadsDestination(A::Difference),
            BlendMode::Exclusion => Blend::ReadsDestination(A::Exclusion),
            BlendMode::Hue => Blend::ReadsDestination(A::Hue),
            BlendMode::Saturation => Blend::ReadsDestination(A::Saturation),
            BlendMode::Color => Blend::ReadsDestination(A::Color),
            BlendMode::Luminosity => Blend::ReadsDestination(A::Luminosity),
        }
    }

    /// `filter_id` is the switch a blend colour filter's fragment takes for
    /// this blend: a pipeline blend's id, or an advanced one's after them.
    pub fn filter_id(self) -> u32 {
        match self {
            Blend::Fixed(blend) => blend.id(),
            Blend::ReadsDestination(blend) => crate::shader_abi::ADVANCED_FILTER_BASE + blend.id(),
        }
    }
}

/// `PipelineBlend` is a blend the blend unit runs: Porter–Duff and the two
/// separable modes, over premultiplied colour.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum PipelineBlend {
    Clear,
    Src,
    Dst,
    SrcOver,
    DstOver,
    SrcIn,
    DstIn,
    SrcOut,
    DstOut,
    SrcAtop,
    DstAtop,
    Xor,
    Plus,
    Modulate,
    Screen,
}

impl PipelineBlend {
    /// `ALL` are the pipeline blends, in id order.
    pub(crate) const ALL: [Self; 15] = [
        Self::Clear,
        Self::Src,
        Self::Dst,
        Self::SrcOver,
        Self::DstOver,
        Self::SrcIn,
        Self::DstIn,
        Self::SrcOut,
        Self::DstOut,
        Self::SrcAtop,
        Self::DstAtop,
        Self::Xor,
        Self::Plus,
        Self::Modulate,
        Self::Screen,
    ];

    /// `id` is the switch `fs_color_blend` takes for this blend.
    pub fn id(self) -> u32 {
        self as u32
    }

    /// `wgsl_name` is the blend's name in the shader's constants.
    pub(crate) fn wgsl_name(self) -> &'static str {
        match self {
            Self::Clear => "CLEAR",
            Self::Src => "SRC",
            Self::Dst => "DST",
            Self::SrcOver => "SRC_OVER",
            Self::DstOver => "DST_OVER",
            Self::SrcIn => "SRC_IN",
            Self::DstIn => "DST_IN",
            Self::SrcOut => "SRC_OUT",
            Self::DstOut => "DST_OUT",
            Self::SrcAtop => "SRC_ATOP",
            Self::DstAtop => "DST_ATOP",
            Self::Xor => "XOR",
            Self::Plus => "PLUS",
            Self::Modulate => "MODULATE",
            Self::Screen => "SCREEN",
        }
    }

    /// `state` is the blend unit's factors for this blend, over
    /// premultiplied colour.
    fn state(self) -> wgpu::BlendState {
        use wgpu::BlendFactor as F;
        let (src, dst) = match self {
            Self::Clear => (F::Zero, F::Zero),
            Self::Src => (F::One, F::Zero),
            Self::Dst => (F::Zero, F::One),
            Self::SrcOver => (F::One, F::OneMinusSrcAlpha),
            Self::DstOver => (F::OneMinusDstAlpha, F::One),
            Self::SrcIn => (F::DstAlpha, F::Zero),
            Self::DstIn => (F::Zero, F::SrcAlpha),
            Self::SrcOut => (F::OneMinusDstAlpha, F::Zero),
            Self::DstOut => (F::Zero, F::OneMinusSrcAlpha),
            Self::SrcAtop => (F::DstAlpha, F::OneMinusSrcAlpha),
            Self::DstAtop => (F::OneMinusDstAlpha, F::SrcAlpha),
            Self::Xor => (F::OneMinusDstAlpha, F::OneMinusSrcAlpha),
            Self::Plus => (F::One, F::One),
            Self::Modulate => (F::Zero, F::Src),
            Self::Screen => (F::One, F::OneMinusSrc),
        };
        let component = wgpu::BlendComponent {
            src_factor: src,
            dst_factor: dst,
            operation: wgpu::BlendOperation::Add,
        };
        wgpu::BlendState {
            color: component,
            alpha: component,
        }
    }
}

/// `AdvancedBlend` is a blend a fragment runs against a copy of the
/// destination (PDF/W3C compositing over unpremultiplied colour).
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum AdvancedBlend {
    Multiply,
    Overlay,
    Darken,
    Lighten,
    ColorDodge,
    ColorBurn,
    HardLight,
    SoftLight,
    Difference,
    Exclusion,
    Hue,
    Saturation,
    Color,
    Luminosity,
}

impl AdvancedBlend {
    /// `ALL` are the advanced blends, in id order.
    pub(crate) const ALL: [Self; 14] = [
        Self::Multiply,
        Self::Overlay,
        Self::Darken,
        Self::Lighten,
        Self::ColorDodge,
        Self::ColorBurn,
        Self::HardLight,
        Self::SoftLight,
        Self::Difference,
        Self::Exclusion,
        Self::Hue,
        Self::Saturation,
        Self::Color,
        Self::Luminosity,
    ];

    /// `id` is the switch the blending fragments take for this blend.
    pub fn id(self) -> u32 {
        self as u32
    }

    /// `wgsl_name` is the blend's name in the shader's constants.
    pub(crate) fn wgsl_name(self) -> &'static str {
        match self {
            Self::Multiply => "MULTIPLY",
            Self::Overlay => "OVERLAY",
            Self::Darken => "DARKEN",
            Self::Lighten => "LIGHTEN",
            Self::ColorDodge => "COLOR_DODGE",
            Self::ColorBurn => "COLOR_BURN",
            Self::HardLight => "HARD_LIGHT",
            Self::SoftLight => "SOFT_LIGHT",
            Self::Difference => "DIFFERENCE",
            Self::Exclusion => "EXCLUSION",
            Self::Hue => "HUE",
            Self::Saturation => "SATURATION",
            Self::Color => "COLOR",
            Self::Luminosity => "LUMINOSITY",
        }
    }
}

/// `PipelineKey` identifies one compiled render pipeline variant.
///
/// The cache keys on surface format, blend, and [`PipelineKind`]. Blend is
/// normalized for kinds that do not blend, so those entries are shared.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub struct PipelineKey {
    pub format: wgpu::TextureFormat,
    pub blend: PipelineBlend,
    pub kind: PipelineKind,
}

impl PipelineKey {
    /// `new` builds a cache key, normalizing `blend` for kinds that replace the destination.
    pub fn new(format: wgpu::TextureFormat, blend: PipelineBlend, kind: PipelineKind) -> Self {
        Self {
            format,
            blend: kind.normalized_blend(blend),
            kind,
        }
    }
}

/// `CompiledPipelines` are the pipelines [`PipelineCache::compile`] has
/// compiled, to draw a frame with.
pub struct CompiledPipelines<'a> {
    map: &'a FxHashMap<PipelineKey, wgpu::RenderPipeline>,
}

impl CompiledPipelines<'_> {
    /// `get` returns the pipeline compiled for `key`, one of the keys the
    /// frame was compiled for.
    pub fn get(&self, key: &PipelineKey) -> &wgpu::RenderPipeline {
        &self.map[key]
    }
}

/// `PipelineCache` holds compiled render-pipeline variants.
///
/// The cache grows only. Misses compile synchronously on first use.
pub struct PipelineCache {
    shader: wgpu::ShaderModule,
    plain_layout: wgpu::PipelineLayout,
    textured_layout: wgpu::PipelineLayout,
    blend_layout: wgpu::PipelineLayout,
    /// A blur pass: the texture at group 1, the kernel at group 2.
    blur_layout: wgpu::PipelineLayout,
    texture_bind_layout: wgpu::BindGroupLayout,
    blend_bind_layout: wgpu::BindGroupLayout,
    map: FxHashMap<PipelineKey, wgpu::RenderPipeline>,
}

impl PipelineCache {
    /// `new` compiles the shader module, its ABI's prelude first, and the
    /// pipeline layouts for `device`.
    ///
    /// `uniforms_layout` and `kernel_layout` are the host buffer's group-0
    /// and group-2 layouts.
    pub fn new(
        device: &wgpu::Device,
        uniforms_layout: &wgpu::BindGroupLayout,
        kernel_layout: &wgpu::BindGroupLayout,
    ) -> Self {
        let source = crate::shader_abi::wgsl_prelude() + include_str!("shaders/solid.wgsl");
        let shader = device.create_shader_module(wgpu::ShaderModuleDescriptor {
            label: Some("valo.solid"),
            source: wgpu::ShaderSource::Wgsl(source.into()),
        });
        let texture_bind_layout = texture_bind_group_layout(device);
        let plain_layout = device.create_pipeline_layout(&wgpu::PipelineLayoutDescriptor {
            label: Some("valo.plain"),
            bind_group_layouts: &[Some(uniforms_layout)],
            immediate_size: 0,
        });
        let textured_layout = device.create_pipeline_layout(&wgpu::PipelineLayoutDescriptor {
            label: Some("valo.textured"),
            bind_group_layouts: &[Some(uniforms_layout), Some(&texture_bind_layout)],
            immediate_size: 0,
        });
        let blend_bind_layout = blend_bind_group_layout(device);
        let blend_layout = device.create_pipeline_layout(&wgpu::PipelineLayoutDescriptor {
            label: Some("valo.blend"),
            bind_group_layouts: &[Some(uniforms_layout), Some(&blend_bind_layout)],
            immediate_size: 0,
        });
        let blur_layout = device.create_pipeline_layout(&wgpu::PipelineLayoutDescriptor {
            label: Some("valo.blur"),
            bind_group_layouts: &[
                Some(uniforms_layout),
                Some(&texture_bind_layout),
                Some(kernel_layout),
            ],
            immediate_size: 0,
        });
        Self {
            shader,
            plain_layout,
            textured_layout,
            blend_layout,
            blur_layout,
            texture_bind_layout,
            blend_bind_layout,
            map: FxHashMap::default(),
        }
    }

    /// `blend_bind_layout` returns the group-1 layout for advanced-blend bind groups.
    ///
    /// Bindings are destination, sampler, and source.
    pub fn blend_bind_layout(&self) -> &wgpu::BindGroupLayout {
        &self.blend_bind_layout
    }

    /// `texture_bind_layout` returns the group-1 layout for image bind groups.
    ///
    /// Bindings are texture and sampler.
    pub fn texture_bind_layout(&self) -> &wgpu::BindGroupLayout {
        &self.texture_bind_layout
    }

    /// `compile` compiles the pipelines for `keys` that are not cached yet
    /// and returns the compiled pipelines to draw with.
    ///
    /// A frame compiles every pipeline its plan names before it encodes:
    /// encoding borrows the pipelines for as long as a render pass records.
    pub fn compile(
        &mut self,
        device: &wgpu::Device,
        keys: impl IntoIterator<Item = PipelineKey>,
    ) -> CompiledPipelines<'_> {
        for key in keys {
            if !self.map.contains_key(&key) {
                let pipeline = self.create(device, key);
                self.map.insert(key, pipeline);
            }
        }
        CompiledPipelines { map: &self.map }
    }

    fn create(&self, device: &wgpu::Device, key: PipelineKey) -> wgpu::RenderPipeline {
        let layout = match key.kind.bind_layout() {
            BindLayout::Plain => &self.plain_layout,
            BindLayout::Textured => &self.textured_layout,
            BindLayout::Blend => &self.blend_layout,
            BindLayout::Blur => &self.blur_layout,
        };
        device.create_render_pipeline(&wgpu::RenderPipelineDescriptor {
            label: Some("valo.solid"),
            layout: Some(layout),
            vertex: wgpu::VertexState {
                module: &self.shader,
                entry_point: Some(key.kind.vertex_entry()),
                compilation_options: Default::default(),
                buffers: vertex_buffers(key.kind),
            },
            fragment: Some(wgpu::FragmentState {
                module: &self.shader,
                entry_point: Some(key.kind.fragment_entry()),
                compilation_options: Default::default(),
                targets: &[Some(color_target(key))],
            }),
            primitive: wgpu::PrimitiveState {
                topology: match key.kind {
                    PipelineKind::Strip(_) | PipelineKind::StencilStrip => {
                        wgpu::PrimitiveTopology::TriangleStrip
                    }
                    _ => wgpu::PrimitiveTopology::TriangleList,
                },
                ..Default::default()
            },
            depth_stencil: depth_stencil(key.kind),
            multisample: wgpu::MultisampleState {
                count: key.kind.sample_count(),
                ..Default::default()
            },
            multiview_mask: None,
            cache: None,
        })
    }
}

fn texture_bind_group_layout(device: &wgpu::Device) -> wgpu::BindGroupLayout {
    device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
        label: Some("valo.texture"),
        entries: &[
            wgpu::BindGroupLayoutEntry {
                binding: 0,
                visibility: wgpu::ShaderStages::FRAGMENT,
                ty: wgpu::BindingType::Texture {
                    sample_type: wgpu::TextureSampleType::Float { filterable: true },
                    view_dimension: wgpu::TextureViewDimension::D2,
                    multisampled: false,
                },
                count: None,
            },
            wgpu::BindGroupLayoutEntry {
                binding: 1,
                visibility: wgpu::ShaderStages::FRAGMENT,
                ty: wgpu::BindingType::Sampler(wgpu::SamplerBindingType::Filtering),
                count: None,
            },
        ],
    })
}

fn blend_bind_group_layout(device: &wgpu::Device) -> wgpu::BindGroupLayout {
    let texture_entry = |binding| wgpu::BindGroupLayoutEntry {
        binding,
        visibility: wgpu::ShaderStages::FRAGMENT,
        ty: wgpu::BindingType::Texture {
            sample_type: wgpu::TextureSampleType::Float { filterable: true },
            view_dimension: wgpu::TextureViewDimension::D2,
            multisampled: false,
        },
        count: None,
    };
    device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
        label: Some("valo.blend"),
        entries: &[
            texture_entry(0), // dst snapshot
            wgpu::BindGroupLayoutEntry {
                binding: 1,
                visibility: wgpu::ShaderStages::FRAGMENT,
                ty: wgpu::BindingType::Sampler(wgpu::SamplerBindingType::Filtering),
                count: None,
            },
            texture_entry(2), // src (layer / desugared draw)
        ],
    })
}

const MESH_LAYOUT: [Option<wgpu::VertexBufferLayout<'static>>; 1] =
    [Some(wgpu::VertexBufferLayout {
        array_stride: (MESH_VERTEX_FLOATS * std::mem::size_of::<f32>()) as u64,
        step_mode: wgpu::VertexStepMode::Vertex,
        attributes: &wgpu::vertex_attr_array![0 => Float32x2],
    })];

const GLYPH_LAYOUT: [Option<wgpu::VertexBufferLayout<'static>>; 1] =
    [Some(wgpu::VertexBufferLayout {
        array_stride: (GLYPH_VERTEX_FLOATS * std::mem::size_of::<f32>()) as u64,
        step_mode: wgpu::VertexStepMode::Vertex,
        attributes: &wgpu::vertex_attr_array![0 => Float32x2, 1 => Float32x2],
    })];

fn vertex_buffers(kind: PipelineKind) -> &'static [Option<wgpu::VertexBufferLayout<'static>>] {
    match kind {
        PipelineKind::StencilFan { .. } | PipelineKind::StencilStrip | PipelineKind::Strip(_) => {
            &MESH_LAYOUT
        }
        PipelineKind::Glyphs(_) => &GLYPH_LAYOUT,
        _ => &[],
    }
}

fn color_target(key: PipelineKey) -> wgpu::ColorTargetState {
    let writes_color = key.kind.writes_color();
    wgpu::ColorTargetState {
        format: key.format,
        blend: key.kind.blends().then(|| key.blend.state()),
        write_mask: if writes_color {
            wgpu::ColorWrites::ALL
        } else {
            wgpu::ColorWrites::empty()
        },
    }
}

/// The depth-clip scheme: depth clears to 0; color draws carry
/// z = their slot and test GreaterEqual — a ceiling written at a clip's expiry
/// blocks in-scope draws (slot < expiry) exactly where the clip excluded them,
/// and later draws (slot > expiry) pass over it. Restores render nothing.
/// A filter pass has no depth attachment.
fn depth_stencil(kind: PipelineKind) -> Option<wgpu::DepthStencilState> {
    let (depth_write_enabled, depth_compare, stencil) = match kind {
        PipelineKind::Draw(_) | PipelineKind::Strip(_) => (
            false,
            wgpu::CompareFunction::GreaterEqual,
            face_pair(ALWAYS_KEEP),
        ),
        // Opaque draws WRITE their z: everything painter-below that they
        // cover fails early-z instead of blending.
        PipelineKind::OpaqueDraw(_) => (
            true,
            wgpu::CompareFunction::GreaterEqual,
            face_pair(ALWAYS_KEEP),
        ),
        PipelineKind::OpaqueCover(_) => (
            true,
            wgpu::CompareFunction::GreaterEqual,
            face_pair(COVER_WOUND),
        ),
        PipelineKind::Cover(_) => (
            false,
            wgpu::CompareFunction::GreaterEqual,
            face_pair(COVER_WOUND),
        ),
        // StC fan: winding into stencil only. NonZero: front faces +1, back
        // faces −1 (holes cancel); EvenOdd: parity by inversion.
        PipelineKind::StencilFan { even_odd } => {
            let winding = |op| wgpu::StencilFaceState {
                compare: wgpu::CompareFunction::Always,
                fail_op: wgpu::StencilOperation::Keep,
                depth_fail_op: wgpu::StencilOperation::Keep,
                pass_op: op,
            };
            let stencil = if even_odd {
                face_pair(winding(wgpu::StencilOperation::Invert))
            } else {
                wgpu::StencilState {
                    front: winding(wgpu::StencilOperation::IncrementWrap),
                    back: winding(wgpu::StencilOperation::DecrementWrap),
                    read_mask: 0xFF,
                    write_mask: 0xFF,
                }
            };
            (false, wgpu::CompareFunction::Always, stencil)
        }
        // A stroke's strip: every triangle counts up, so overlaps stay
        // marked (Impeller's `kStencilIncrementAll`).
        PipelineKind::StencilStrip => (
            false,
            wgpu::CompareFunction::Always,
            face_pair(wgpu::StencilFaceState {
                compare: wgpu::CompareFunction::Always,
                fail_op: wgpu::StencilOperation::Keep,
                depth_fail_op: wgpu::StencilOperation::Keep,
                pass_op: wgpu::StencilOperation::IncrementClamp,
            }),
        ),
        PipelineKind::Filter(_) => return None,
        // Glyph quads depth-test like any draw (clips apply, no writes).
        PipelineKind::Glyphs(_) => (
            false,
            wgpu::CompareFunction::GreaterEqual,
            face_pair(ALWAYS_KEEP),
        ),
        // Clip ceiling: write z=expiry where covered. Compare Greater (only
        // ever raise: an inner clip's earlier expiry must not overwrite an
        // outer clip's later one). Every stencil outcome zeroes — the cover
        // is also the stencil reset.
        PipelineKind::ClipCover { difference } => (
            true,
            wgpu::CompareFunction::Greater,
            face_pair(wgpu::StencilFaceState {
                compare: if difference {
                    wgpu::CompareFunction::NotEqual // ceiling INSIDE the shape
                } else {
                    wgpu::CompareFunction::Equal // ceiling OUTSIDE the shape
                },
                fail_op: wgpu::StencilOperation::Zero,
                depth_fail_op: wgpu::StencilOperation::Zero,
                pass_op: wgpu::StencilOperation::Zero,
            }),
        ),
    };
    Some(wgpu::DepthStencilState {
        format: DEPTH_FORMAT,
        depth_write_enabled: Some(depth_write_enabled),
        depth_compare: Some(depth_compare),
        stencil,
        bias: Default::default(),
    })
}

/// `COVER_WOUND` is stencil-then-cover's cover: draw where wound (stencil
/// != 0), resetting stencil to 0 behind itself so the next path starts clean
/// — even where the depth clip rejects the pixel (depth_fail still zeroes).
const COVER_WOUND: wgpu::StencilFaceState = wgpu::StencilFaceState {
    compare: wgpu::CompareFunction::NotEqual,
    fail_op: wgpu::StencilOperation::Keep,
    depth_fail_op: wgpu::StencilOperation::Zero,
    pass_op: wgpu::StencilOperation::Zero,
};

const ALWAYS_KEEP: wgpu::StencilFaceState = wgpu::StencilFaceState {
    compare: wgpu::CompareFunction::Always,
    fail_op: wgpu::StencilOperation::Keep,
    depth_fail_op: wgpu::StencilOperation::Keep,
    pass_op: wgpu::StencilOperation::Keep,
};

fn face_pair(face: wgpu::StencilFaceState) -> wgpu::StencilState {
    wgpu::StencilState {
        front: face,
        back: face,
        read_mask: 0xFF,
        write_mask: 0xFF,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Every blend mode is run one way, and each kind's ids follow its
    /// table's order.
    #[test]
    fn every_blend_mode_is_run_by_the_blend_unit_or_the_fragment() {
        assert_eq!(
            Blend::of(BlendMode::Screen),
            Blend::Fixed(PipelineBlend::Screen)
        );
        assert_eq!(
            Blend::of(BlendMode::Luminosity),
            Blend::ReadsDestination(AdvancedBlend::Luminosity)
        );
        for (id, blend) in PipelineBlend::ALL.into_iter().enumerate() {
            assert_eq!(blend.id(), id as u32);
        }
        for (id, blend) in AdvancedBlend::ALL.into_iter().enumerate() {
            assert_eq!(blend.id(), id as u32);
        }
        assert_eq!(Blend::of(BlendMode::Multiply).filter_id(), 15);
    }
}
