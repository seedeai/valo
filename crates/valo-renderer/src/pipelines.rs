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

/// `Frag` selects the fragment shader that colors a covered pixel.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum Frag {
    Solid,
    Image,
    /// Direct-image color filters run after texture sampling.
    ImageMatrix,
    ImageBlend,
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
    /// One direction of a separable gaussian (filter passes only).
    Blur,
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
    /// matrix, or a constant colour blended as the source.
    ColorMatrix,
    ColorBlend,
    /// An image tiled across the shape, sampled through the paint's own
    /// local matrix — Canvas2D's pattern.
    Pattern,
}

impl Frag {
    fn entry_point(self) -> &'static str {
        match self {
            Frag::Solid => "fs_solid",
            Frag::Image => "fs_image",
            Frag::ImageMatrix => "fs_image_matrix",
            Frag::ImageBlend => "fs_image_blend",
            Frag::Linear => "fs_linear",
            Frag::Radial => "fs_radial",
            Frag::Sweep => "fs_sweep",
            Frag::BlendSolid => "fs_blend_solid",
            Frag::BlendTexture => "fs_blend_texture",
            Frag::RRectBlur => "fs_rrect_blur",
            Frag::Blur => "fs_blur",
            Frag::MaskCombine => "fs_mask_combine",
            Frag::DropShadow => "fs_drop_shadow",
            Frag::MaskComposite => "fs_mask_composite",
            Frag::LinearRamp => "fs_linear_ramp",
            Frag::RadialRamp => "fs_radial_ramp",
            Frag::SweepRamp => "fs_sweep_ramp",
            Frag::ColorMatrix => "fs_color_matrix",
            Frag::ColorBlend => "fs_color_blend",
            Frag::Pattern => "fs_pattern",
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
    /// Depth-clip ceiling: z=expiry written outside (Intersect) or
    /// inside (Difference) the stenciled shape; no color.
    ClipCover { difference: bool },
    /// Bare color work between the frame's passes (gaussian blur chains):
    /// 1-sample, no depth/stencil, output replaces the target.
    Filter(Frag),
    /// Stroke geometry: a CPU triangle STRIP along the path;
    /// depth-tested like a draw, any fragment family composes.
    Strip(Frag),
    /// Atlas-masked glyph quads (pos + uv vertices).
    Text { mode: TextMode },
}

/// `TextMode` selects how a glyph quad reads its atlas page.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum TextMode {
    /// R8 coverage × tint (the pixel-aligned bitmap tier).
    Mask,
    /// R8 distance field thresholded at 0.5 (the transformed tier).
    Sdf,
    /// RGBA color glyphs (emoji) × alpha-only tint.
    Color,
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
                | PipelineKind::Text { .. }
        )
    }

    fn frag(self) -> Option<Frag> {
        match self {
            PipelineKind::Draw(f)
            | PipelineKind::Cover(f)
            | PipelineKind::OpaqueDraw(f)
            | PipelineKind::OpaqueCover(f)
            | PipelineKind::Filter(f)
            | PipelineKind::Strip(f) => Some(f),
            _ => None,
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
        if let PipelineKind::Text { mode } = self {
            return match mode {
                TextMode::Mask => "fs_text",
                TextMode::Sdf => "fs_text_sdf",
                TextMode::Color => "fs_text_color",
            };
        }
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
            PipelineKind::StencilFan { .. } | PipelineKind::Strip(_) => "vs_mesh",
            PipelineKind::Text { .. } => "vs_text",
            _ => "vs_quad",
        }
    }

    /// Blend only matters where color is written AND blended; normalizing
    /// the rest de-duplicates cache entries.
    fn normalized_blend(self, blend: BlendMode) -> BlendMode {
        if self.writes_color() && !self.replaces_dst() {
            blend
        } else {
            BlendMode::SrcOver
        }
    }
}

/// `PipelineKey` identifies one compiled render pipeline variant.
///
/// The cache keys on surface format, blend mode, and [`PipelineKind`]. Blend
/// is normalized for kinds that do not blend, so those entries are shared.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub struct PipelineKey {
    pub format: wgpu::TextureFormat,
    pub blend: BlendMode,
    pub kind: PipelineKind,
}

impl PipelineKey {
    /// `new` builds a cache key, normalizing `blend` for kinds that replace the destination.
    pub fn new(format: wgpu::TextureFormat, blend: BlendMode, kind: PipelineKind) -> Self {
        Self {
            format,
            blend: kind.normalized_blend(blend),
            kind,
        }
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
    texture_bind_layout: wgpu::BindGroupLayout,
    blend_bind_layout: wgpu::BindGroupLayout,
    map: FxHashMap<PipelineKey, wgpu::RenderPipeline>,
}

impl PipelineCache {
    /// `new` compiles the shader module and pipeline layouts for `device`.
    pub fn new(device: &wgpu::Device, uniforms_layout: &wgpu::BindGroupLayout) -> Self {
        let shader = device.create_shader_module(wgpu::ShaderModuleDescriptor {
            label: Some("valo.solid"),
            source: wgpu::ShaderSource::Wgsl(include_str!("shaders/solid.wgsl").into()),
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
        Self {
            shader,
            plain_layout,
            textured_layout,
            blend_layout,
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

    /// `ensure` compiles the pipeline for `key` if it is not already cached.
    pub fn ensure(&mut self, device: &wgpu::Device, key: PipelineKey) {
        if !self.map.contains_key(&key) {
            let pipeline = self.create(device, key);
            self.map.insert(key, pipeline);
        }
    }

    /// `get` returns a pipeline previously compiled by [`Self::ensure`].
    ///
    /// Panics if `key` was never ensured.
    pub fn get(&self, key: &PipelineKey) -> &wgpu::RenderPipeline {
        &self.map[key]
    }

    fn create(&self, device: &wgpu::Device, key: PipelineKey) -> wgpu::RenderPipeline {
        let layout = match key.kind.frag() {
            _ if matches!(key.kind, PipelineKind::Text { .. }) => &self.textured_layout,
            Some(Frag::BlendTexture) | Some(Frag::MaskCombine) | Some(Frag::DropShadow) => {
                &self.blend_layout
            }
            Some(Frag::Image)
            | Some(Frag::ImageMatrix)
            | Some(Frag::ImageBlend)
            | Some(Frag::BlendSolid)
            | Some(Frag::Blur)
            | Some(Frag::MaskComposite)
            | Some(Frag::LinearRamp)
            | Some(Frag::RadialRamp)
            | Some(Frag::SweepRamp)
            | Some(Frag::ColorMatrix)
            | Some(Frag::ColorBlend)
            | Some(Frag::Pattern) => &self.textured_layout,
            _ => &self.plain_layout,
        };
        let depth_stencil = match key.kind {
            PipelineKind::Filter(_) => None,
            kind => Some(depth_stencil(kind)),
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
                    PipelineKind::Strip(_) => wgpu::PrimitiveTopology::TriangleStrip,
                    _ => wgpu::PrimitiveTopology::TriangleList,
                },
                ..Default::default()
            },
            depth_stencil,
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

/// `blur_style_id` maps a blur style to the shader switch used by rounded-rect
/// blur and mask-combine passes.
pub fn blur_style_id(style: valo_dl::BlurStyle) -> u32 {
    match style {
        valo_dl::BlurStyle::Normal => 0,
        valo_dl::BlurStyle::Solid => 1,
        valo_dl::BlurStyle::Inner => 2,
        valo_dl::BlurStyle::Outer => 3,
    }
}

/// `blend_filter_id` maps a blend mode to the switch used by `fs_color_blend`.
///
/// Porter-Duff and the two separable modes occupy 0–14. Destination-reading
/// (advanced) modes follow at 15 plus [`advanced_mode_id`].
pub fn blend_filter_id(mode: BlendMode) -> u32 {
    match mode {
        BlendMode::Clear => 0,
        BlendMode::Src => 1,
        BlendMode::Dst => 2,
        BlendMode::SrcOver => 3,
        BlendMode::DstOver => 4,
        BlendMode::SrcIn => 5,
        BlendMode::DstIn => 6,
        BlendMode::SrcOut => 7,
        BlendMode::DstOut => 8,
        BlendMode::SrcAtop => 9,
        BlendMode::DstAtop => 10,
        BlendMode::Xor => 11,
        BlendMode::Plus => 12,
        BlendMode::Modulate => 13,
        BlendMode::Screen => 14,
        advanced => 15 + advanced_mode_id(advanced),
    }
}

/// `advanced_mode_id` maps a destination-reading blend mode to the shader switch.
///
/// Panics if `mode` is a pipeline-blendable (Porter-Duff / separable) mode.
pub fn advanced_mode_id(mode: BlendMode) -> u32 {
    match mode {
        BlendMode::Multiply => 0,
        BlendMode::Overlay => 1,
        BlendMode::Darken => 2,
        BlendMode::Lighten => 3,
        BlendMode::ColorDodge => 4,
        BlendMode::ColorBurn => 5,
        BlendMode::HardLight => 6,
        BlendMode::SoftLight => 7,
        BlendMode::Difference => 8,
        BlendMode::Exclusion => 9,
        BlendMode::Hue => 10,
        BlendMode::Saturation => 11,
        BlendMode::Color => 12,
        BlendMode::Luminosity => 13,
        _ => unreachable!("pipeline-blendable mode routed to advanced path"),
    }
}

const MESH_LAYOUT: [Option<wgpu::VertexBufferLayout<'static>>; 1] =
    [Some(wgpu::VertexBufferLayout {
        array_stride: 8,
        step_mode: wgpu::VertexStepMode::Vertex,
        attributes: &wgpu::vertex_attr_array![0 => Float32x2],
    })];

const TEXT_LAYOUT: [Option<wgpu::VertexBufferLayout<'static>>; 1] =
    [Some(wgpu::VertexBufferLayout {
        array_stride: 16,
        step_mode: wgpu::VertexStepMode::Vertex,
        attributes: &wgpu::vertex_attr_array![0 => Float32x2, 1 => Float32x2],
    })];

fn vertex_buffers(kind: PipelineKind) -> &'static [Option<wgpu::VertexBufferLayout<'static>>] {
    match kind {
        PipelineKind::StencilFan { .. } | PipelineKind::Strip(_) => &MESH_LAYOUT,
        PipelineKind::Text { .. } => &TEXT_LAYOUT,
        _ => &[],
    }
}

fn color_target(key: PipelineKey) -> wgpu::ColorTargetState {
    let writes_color = key.kind.writes_color();
    wgpu::ColorTargetState {
        format: key.format,
        blend: (writes_color && !key.kind.replaces_dst()).then(|| blend_state(key.blend)),
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
fn depth_stencil(kind: PipelineKind) -> wgpu::DepthStencilState {
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
            face_pair(wgpu::StencilFaceState {
                compare: wgpu::CompareFunction::NotEqual,
                fail_op: wgpu::StencilOperation::Keep,
                depth_fail_op: wgpu::StencilOperation::Zero,
                pass_op: wgpu::StencilOperation::Zero,
            }),
        ),
        // StC cover: draw where wound (stencil != 0), resetting stencil to 0
        // behind itself so the next path starts clean — even where the depth
        // clip rejects the pixel (depth_fail still zeroes).
        PipelineKind::Cover(_) => (
            false,
            wgpu::CompareFunction::GreaterEqual,
            face_pair(wgpu::StencilFaceState {
                compare: wgpu::CompareFunction::NotEqual,
                fail_op: wgpu::StencilOperation::Keep,
                depth_fail_op: wgpu::StencilOperation::Zero,
                pass_op: wgpu::StencilOperation::Zero,
            }),
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
        // Clip ceiling: write z=expiry where covered. Compare Greater (only
        // ever raise: an inner clip's earlier expiry must not overwrite an
        // outer clip's later one). Every stencil outcome zeroes — the cover
        // is also the stencil reset.
        PipelineKind::Filter(_) => unreachable!("filter passes carry no depth attachment"),
        // Glyph quads depth-test like any draw (clips apply, no writes).
        PipelineKind::Text { .. } => (
            false,
            wgpu::CompareFunction::GreaterEqual,
            face_pair(ALWAYS_KEEP),
        ),
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
    wgpu::DepthStencilState {
        format: DEPTH_FORMAT,
        depth_write_enabled: Some(depth_write_enabled),
        depth_compare: Some(depth_compare),
        stencil,
        bias: Default::default(),
    }
}

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

/// Porter–Duff over PREMULTIPLIED color. The dst-reading advanced modes are not
/// pipeline-expressible; callers map them to `SrcOver` before asking (M4 brings
/// the real machinery).
fn blend_state(mode: BlendMode) -> wgpu::BlendState {
    use wgpu::BlendFactor as F;
    let (src, dst) = match mode {
        BlendMode::Clear => (F::Zero, F::Zero),
        BlendMode::Src => (F::One, F::Zero),
        BlendMode::Dst => (F::Zero, F::One),
        BlendMode::SrcOver => (F::One, F::OneMinusSrcAlpha),
        BlendMode::DstOver => (F::OneMinusDstAlpha, F::One),
        BlendMode::SrcIn => (F::DstAlpha, F::Zero),
        BlendMode::DstIn => (F::Zero, F::SrcAlpha),
        BlendMode::SrcOut => (F::OneMinusDstAlpha, F::Zero),
        BlendMode::DstOut => (F::Zero, F::OneMinusSrcAlpha),
        BlendMode::SrcAtop => (F::DstAlpha, F::OneMinusSrcAlpha),
        BlendMode::DstAtop => (F::OneMinusDstAlpha, F::SrcAlpha),
        BlendMode::Xor => (F::OneMinusDstAlpha, F::OneMinusSrcAlpha),
        BlendMode::Plus => (F::One, F::One),
        BlendMode::Modulate => (F::Zero, F::Src),
        BlendMode::Screen => (F::One, F::OneMinusSrc),
        // Advanced modes were mapped to SrcOver upstream; keep a total match
        // so a slipped-through key still renders deterministically.
        _ => (F::One, F::OneMinusSrcAlpha),
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
