use std::collections::HashMap;
use std::sync::Weak;

use valo_dl::{BlendMode, ColorFilter, Filter, Image, ImageInner, Sampling, TileMode};

/// How pixels arrive at `upload`.
#[derive(Clone, Copy, Debug)]
pub struct ImageDesc {
    pub size: [u32; 2],
    /// `false` = straight alpha: premultiplied on the CPU at the boundary
    /// (Skia's kPremul convention — everything downstream assumes premul).
    pub premultiplied: bool,
    /// Build a full mip chain (posters downscale constantly; mips are the
    /// difference between shimmer and smooth).
    pub mips: bool,
}

impl Default for ImageDesc {
    fn default() -> Self {
        Self {
            size: [0, 0],
            premultiplied: false,
            mips: true,
        }
    }
}

/// Everything image-shaped the renderer owns: upload (+CPU premultiply),
/// GPU mip generation, the sampler cache, and the per-(image, sampling)
/// bind-group cache. Bind groups are created once and reused — per-frame
/// creates are the wasm cost to avoid; dead images are swept via `Weak`.
pub struct ImageStore {
    device: wgpu::Device,
    queue: wgpu::Queue,
    samplers: HashMap<Sampling, wgpu::Sampler>,
    binds: HashMap<(u64, Sampling), (Weak<ImageInner>, wgpu::BindGroup)>,
    filtered: HashMap<(u64, ColorFilterKey), FilteredImage>,
    frame: u64,
    mips: MipGenerator,
}

pub const IMAGE_FORMAT: wgpu::TextureFormat = wgpu::TextureFormat::Rgba8Unorm;

impl ImageStore {
    pub fn new(device: &wgpu::Device, queue: &wgpu::Queue) -> Self {
        Self {
            device: device.clone(),
            queue: queue.clone(),
            samplers: HashMap::new(),
            binds: HashMap::new(),
            filtered: HashMap::new(),
            frame: 0,
            mips: MipGenerator::new(device),
        }
    }

    /// The immutable texture that represents `image` after `filter`. The
    /// caller records the producing pass only when `created` is true.
    pub fn filtered_image(&mut self, image: &Image, filter: ColorFilter) -> (Image, bool) {
        self.sweep_if_crowded();
        let key = (image.id(), ColorFilterKey::from(filter));
        if let Some(entry) = self.filtered.get_mut(&key) {
            entry.last_used = self.frame;
            return (entry.image.clone(), false);
        }
        let texture = self.create_image_texture(image.size(), 1);
        let filtered = Image::from_texture(texture, image.size(), 1);
        self.filtered.insert(
            key,
            FilteredImage {
                source: image.downgrade(),
                image: filtered.clone(),
                last_used: self.frame,
            },
        );
        (filtered, true)
    }

    /// One idle frame releases a filtered snapshot even when the host keeps
    /// its source image alive, bounding retention to the visible working set.
    pub fn end_frame(&mut self) {
        let current = self.frame;
        self.frame += 1;
        let before = self.filtered.len();
        self.filtered
            .retain(|_, entry| entry.last_used >= current && entry.source.strong_count() > 0);
        if self.filtered.len() != before {
            // Bind groups retain texture views. Drop dead ones now so cache
            // eviction releases the corresponding GPU textures promptly.
            self.binds.retain(|_, (weak, _)| weak.strong_count() > 0);
        }
    }

    /// RGBA8 pixels → retained [`Image`]. Premultiplies if needed, writes
    /// level 0, renders the mip chain.
    pub fn upload(&mut self, desc: ImageDesc, pixels: &[u8]) -> Image {
        let [w, h] = desc.size;
        assert_eq!(pixels.len(), (w * h * 4) as usize, "RGBA8 pixel count");
        let premul = premultiplied_pixels(desc.premultiplied, pixels);
        let mip_levels = if desc.mips {
            full_mip_count(desc.size)
        } else {
            1
        };
        let texture = self.create_image_texture(desc.size, mip_levels);
        self.write_level_zero(&texture, desc.size, &premul);
        if mip_levels > 1 {
            self.mips
                .generate(&self.device, &self.queue, &texture, desc.size, mip_levels);
        }
        Image::from_texture(texture, desc.size, mip_levels)
    }

    /// Wrap an already-populated texture (the web `ImageBitmap` path copies
    /// externally, then generates mips here).
    pub fn finish_external(
        &mut self,
        texture: wgpu::Texture,
        size: [u32; 2],
        mip_levels: u32,
    ) -> Image {
        if mip_levels > 1 {
            self.mips
                .generate(&self.device, &self.queue, &texture, size, mip_levels);
        }
        Image::from_texture(texture, size, mip_levels)
    }

    pub fn create_image_texture(&self, size: [u32; 2], mip_levels: u32) -> wgpu::Texture {
        self.device.create_texture(&wgpu::TextureDescriptor {
            label: Some("valo.image"),
            size: wgpu::Extent3d {
                width: size[0],
                height: size[1],
                depth_or_array_layers: 1,
            },
            mip_level_count: mip_levels,
            sample_count: 1,
            dimension: wgpu::TextureDimension::D2,
            format: IMAGE_FORMAT,
            // RENDER_ATTACHMENT: mip levels are generated by rendering into them.
            usage: wgpu::TextureUsages::TEXTURE_BINDING
                | wgpu::TextureUsages::COPY_DST
                | wgpu::TextureUsages::RENDER_ATTACHMENT,
            view_formats: &[],
        })
    }

    /// The cached (texture, sampler) bind group for a draw.
    pub fn bind_group(
        &mut self,
        texture_layout: &wgpu::BindGroupLayout,
        image: &Image,
        sampling: Sampling,
    ) -> wgpu::BindGroup {
        self.sweep_if_crowded();
        let key = (image.id(), sampling);
        if let Some((_, bind)) = self.binds.get(&key) {
            return bind.clone();
        }
        let sampler = self.sampler(sampling).clone();
        let bind = self.device.create_bind_group(&wgpu::BindGroupDescriptor {
            label: Some("valo.image"),
            layout: texture_layout,
            entries: &[
                wgpu::BindGroupEntry {
                    binding: 0,
                    resource: wgpu::BindingResource::TextureView(image.view()),
                },
                wgpu::BindGroupEntry {
                    binding: 1,
                    resource: wgpu::BindingResource::Sampler(&sampler),
                },
            ],
        });
        self.binds.insert(key, (image.downgrade(), bind.clone()));
        bind
    }

    fn sampler(&mut self, sampling: Sampling) -> &wgpu::Sampler {
        self.samplers.entry(sampling).or_insert_with(|| {
            let (filter, mip_filter) = match sampling.filter {
                Filter::Linear => (wgpu::FilterMode::Linear, wgpu::MipmapFilterMode::Linear),
                Filter::Nearest => (wgpu::FilterMode::Nearest, wgpu::MipmapFilterMode::Nearest),
            };
            self.device.create_sampler(&wgpu::SamplerDescriptor {
                label: Some("valo.image"),
                address_mode_u: address_mode(sampling.tile_x),
                address_mode_v: address_mode(sampling.tile_y),
                mag_filter: filter,
                min_filter: filter,
                mipmap_filter: mip_filter,
                ..Default::default()
            })
        })
    }

    fn write_level_zero(&self, texture: &wgpu::Texture, size: [u32; 2], premul: &[u8]) {
        self.queue.write_texture(
            wgpu::TexelCopyTextureInfo {
                texture,
                mip_level: 0,
                origin: wgpu::Origin3d::ZERO,
                aspect: wgpu::TextureAspect::All,
            },
            premul,
            wgpu::TexelCopyBufferLayout {
                offset: 0,
                bytes_per_row: Some(size[0] * 4),
                rows_per_image: None,
            },
            wgpu::Extent3d {
                width: size[0],
                height: size[1],
                depth_or_array_layers: 1,
            },
        );
    }

    /// Bind groups whose image died are dropped; runs only when the cache
    /// grows past a threshold (posters hold tens of images, not thousands).
    fn sweep_if_crowded(&mut self) {
        if self.binds.len() > 256 {
            self.binds.retain(|_, (weak, _)| weak.strong_count() > 0);
        }
    }
}

struct FilteredImage {
    source: Weak<ImageInner>,
    image: Image,
    last_used: u64,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
enum ColorFilterKey {
    Matrix([u32; 20]),
    Blend([u32; 4], BlendMode),
}

impl From<ColorFilter> for ColorFilterKey {
    fn from(filter: ColorFilter) -> Self {
        match filter {
            ColorFilter::Matrix(matrix) => Self::Matrix(matrix.map(f32::to_bits)),
            ColorFilter::Blend(color, mode) => Self::Blend(
                [
                    color.r.to_bits(),
                    color.g.to_bits(),
                    color.b.to_bits(),
                    color.a.to_bits(),
                ],
                mode,
            ),
        }
    }
}

fn address_mode(tile: TileMode) -> wgpu::AddressMode {
    match tile {
        TileMode::Clamp => wgpu::AddressMode::ClampToEdge,
        TileMode::Repeat => wgpu::AddressMode::Repeat,
        TileMode::Mirror => wgpu::AddressMode::MirrorRepeat,
    }
}

fn premultiplied_pixels(already: bool, pixels: &[u8]) -> std::borrow::Cow<'_, [u8]> {
    if already {
        return std::borrow::Cow::Borrowed(pixels);
    }
    let mut out = pixels.to_vec();
    for px in out.chunks_exact_mut(4) {
        let a = px[3] as u32;
        px[0] = ((px[0] as u32 * a) / 255) as u8;
        px[1] = ((px[1] as u32 * a) / 255) as u8;
        px[2] = ((px[2] as u32 * a) / 255) as u8;
    }
    std::borrow::Cow::Owned(out)
}

fn full_mip_count(size: [u32; 2]) -> u32 {
    32 - size[0].max(size[1]).max(1).leading_zeros()
}

/// Renders each mip level from the one above (fullscreen triangle + linear
/// sample). Runs once per upload — never per frame.
struct MipGenerator {
    pipeline: wgpu::RenderPipeline,
    layout: wgpu::BindGroupLayout,
    sampler: wgpu::Sampler,
}

const MIP_SHADER: &str = r#"
struct VsOut { @builtin(position) pos: vec4<f32>, @location(0) uv: vec2<f32> };
@vertex fn vs(@builtin(vertex_index) vi: u32) -> VsOut {
    // Fullscreen triangle.
    let xy = vec2<f32>(f32((vi << 1u) & 2u), f32(vi & 2u));
    var out: VsOut;
    out.pos = vec4<f32>(xy * 2.0 - 1.0, 0.0, 1.0);
    out.uv = vec2<f32>(xy.x, 1.0 - xy.y);
    return out;
}
@group(0) @binding(0) var t: texture_2d<f32>;
@group(0) @binding(1) var s: sampler;
@fragment fn fs(in: VsOut) -> @location(0) vec4<f32> {
    return textureSample(t, s, in.uv);
}
"#;

impl MipGenerator {
    fn new(device: &wgpu::Device) -> Self {
        let shader = device.create_shader_module(wgpu::ShaderModuleDescriptor {
            label: Some("valo.mips"),
            source: wgpu::ShaderSource::Wgsl(MIP_SHADER.into()),
        });
        let layout = device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
            label: Some("valo.mips"),
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
        });
        let pipeline_layout = device.create_pipeline_layout(&wgpu::PipelineLayoutDescriptor {
            label: Some("valo.mips"),
            bind_group_layouts: &[Some(&layout)],
            immediate_size: 0,
        });
        let pipeline = device.create_render_pipeline(&wgpu::RenderPipelineDescriptor {
            label: Some("valo.mips"),
            layout: Some(&pipeline_layout),
            vertex: wgpu::VertexState {
                module: &shader,
                entry_point: Some("vs"),
                compilation_options: Default::default(),
                buffers: &[],
            },
            fragment: Some(wgpu::FragmentState {
                module: &shader,
                entry_point: Some("fs"),
                compilation_options: Default::default(),
                targets: &[Some(wgpu::ColorTargetState {
                    format: IMAGE_FORMAT,
                    blend: None,
                    write_mask: wgpu::ColorWrites::ALL,
                })],
            }),
            primitive: wgpu::PrimitiveState::default(),
            depth_stencil: None,
            multisample: wgpu::MultisampleState::default(),
            multiview_mask: None,
            cache: None,
        });
        let sampler = device.create_sampler(&wgpu::SamplerDescriptor {
            label: Some("valo.mips"),
            mag_filter: wgpu::FilterMode::Linear,
            min_filter: wgpu::FilterMode::Linear,
            ..Default::default()
        });
        Self {
            pipeline,
            layout,
            sampler,
        }
    }

    fn generate(
        &self,
        device: &wgpu::Device,
        queue: &wgpu::Queue,
        texture: &wgpu::Texture,
        _size: [u32; 2],
        mip_levels: u32,
    ) {
        let mut encoder = device.create_command_encoder(&wgpu::CommandEncoderDescriptor {
            label: Some("valo.mips"),
        });
        for level in 1..mip_levels {
            let (src, dst) = (
                self.level_view(texture, level - 1),
                self.level_view(texture, level),
            );
            let bind = device.create_bind_group(&wgpu::BindGroupDescriptor {
                label: Some("valo.mips"),
                layout: &self.layout,
                entries: &[
                    wgpu::BindGroupEntry {
                        binding: 0,
                        resource: wgpu::BindingResource::TextureView(&src),
                    },
                    wgpu::BindGroupEntry {
                        binding: 1,
                        resource: wgpu::BindingResource::Sampler(&self.sampler),
                    },
                ],
            });
            self.blit_level(&mut encoder, &dst, &bind);
        }
        queue.submit(std::iter::once(encoder.finish()));
    }

    fn level_view(&self, texture: &wgpu::Texture, level: u32) -> wgpu::TextureView {
        texture.create_view(&wgpu::TextureViewDescriptor {
            base_mip_level: level,
            mip_level_count: Some(1),
            ..Default::default()
        })
    }

    fn blit_level(
        &self,
        encoder: &mut wgpu::CommandEncoder,
        dst: &wgpu::TextureView,
        bind: &wgpu::BindGroup,
    ) {
        let mut pass = encoder.begin_render_pass(&wgpu::RenderPassDescriptor {
            label: Some("valo.mips"),
            color_attachments: &[Some(wgpu::RenderPassColorAttachment {
                view: dst,
                depth_slice: None,
                resolve_target: None,
                ops: wgpu::Operations {
                    load: wgpu::LoadOp::Clear(wgpu::Color::TRANSPARENT),
                    store: wgpu::StoreOp::Store,
                },
            })],
            depth_stencil_attachment: None,
            timestamp_writes: None,
            occlusion_query_set: None,
            multiview_mask: None,
        });
        pass.set_pipeline(&self.pipeline);
        pass.set_bind_group(0, bind, &[]);
        pass.draw(0..3, 0..1);
    }
}

impl ImageStore {
    /// Live uploaded images, deduped across sampler variants; bytes cover
    /// the mip chain (a full chain adds ~1/3).
    pub(crate) fn report(&self) -> crate::PoolReport {
        let mut seen = std::collections::HashSet::new();
        let mut bytes = 0u64;
        for (weak, _) in self.binds.values() {
            let Some(inner) = weak.upgrade() else {
                continue;
            };
            if !seen.insert(inner.id) {
                continue;
            }
            let base = inner.size[0] as u64 * inner.size[1] as u64 * 4;
            bytes += if inner.mip_levels > 1 {
                base * 4 / 3
            } else {
                base
            };
        }
        crate::PoolReport {
            count: seen.len() as u32,
            bytes,
        }
    }
}
