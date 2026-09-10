use crate::ImageContext;
use std::collections::HashMap;
use std::sync::Weak;

use valo_dl::{BlendMode, ColorFilter, Filter, Image, ImageInner, MipmapMode, Sampling, TileMode};

/// `ImageDesc` describes an RGBA8 image upload.
#[derive(Clone, Copy, Debug)]
pub struct ImageDesc {
    /// `size` is the image dimensions in pixels.
    pub size: [u32; 2],
    /// `premultiplied` indicates whether the supplied RGB channels already
    /// contain alpha multiplication.
    ///
    /// When `false`, Valo premultiplies them during upload.
    pub premultiplied: bool,
    /// `mips` controls whether Valo builds a full mip chain.
    ///
    /// Enable it when the image may be drawn smaller than its source size.
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

/// `ImageStore` caches image samplers, bind groups and filtered snapshots.
///
/// Bind groups are created once per (image, sampling) pair and reused. Dead
/// images are swept via `Weak` so the store does not pin host-dropped images.
pub struct ImageStore {
    device: wgpu::Device,
    samplers: HashMap<Sampling, wgpu::Sampler>,
    binds: HashMap<(u64, Sampling), (Weak<ImageInner>, wgpu::BindGroup)>,
    filtered: HashMap<(u64, ColorFilterKey), FilteredImage>,
    frame: u64,
    pub(crate) context: ImageContext,
}

/// `IMAGE_FORMAT` is the GPU format of every uploaded image texture.
pub const IMAGE_FORMAT: wgpu::TextureFormat = wgpu::TextureFormat::Rgba8Unorm;

impl ImageStore {
    /// `new` creates an empty image store for `device` and `queue`.
    pub fn new(device: &wgpu::Device, queue: &wgpu::Queue) -> Self {
        Self {
            device: device.clone(),
            samplers: HashMap::new(),
            binds: HashMap::new(),
            filtered: HashMap::new(),
            frame: 0,
            context: ImageContext::new(device.clone(), queue.clone()),
        }
    }

    /// `filtered_image` returns the immutable texture that represents `image` after `filter`.
    ///
    /// The second return value is `true` when this call created the texture.
    /// The caller records the producing pass only in that case.
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

    /// `end_frame` drops filtered snapshots unused this frame.
    ///
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

    /// `upload` creates a retained [`Image`] from RGBA8 pixels.
    ///
    /// Premultiplies when `desc.premultiplied` is false, writes mip level 0,
    /// and builds the mip chain when `desc.mips` is true. Panics if
    /// `pixels.len()` is not `width * height * 4`.
    pub fn upload(&self, desc: ImageDesc, pixels: &[u8]) -> Image {
        self.context.upload_rgba(desc, pixels)
    }

    /// `finish_external` wraps an already-populated texture as a retained [`Image`].
    ///
    /// Use this when the host copied pixels itself (for example an
    /// `ImageBitmap` upload). Builds the mip chain when `mip_levels` is
    /// greater than 1.
    pub fn finish_external(
        &self,
        texture: wgpu::Texture,
        size: [u32; 2],
        mip_levels: u32,
    ) -> Image {
        self.context.finish_external(texture, size, mip_levels)
    }

    /// `regenerate_mips` rebuilds the mip chain after level 0 was rewritten in place.
    ///
    /// Call this after each copy from a per-frame source such as a video frame.
    pub fn regenerate_mips(&self, image: &Image) {
        self.context.regenerate_mips(image)
    }

    /// `create_image_texture` allocates an empty image texture of `size` and `mip_levels`.
    ///
    /// The texture is bindable, copy-destination, and a render attachment so
    /// mip levels can be generated by rendering into them.
    pub fn create_image_texture(&self, size: [u32; 2], mip_levels: u32) -> wgpu::Texture {
        self.context.create_image_texture(size, mip_levels)
    }

    /// `bind_group` returns the cached (texture, sampler) bind group for a draw.
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
            let filter = match sampling.filter {
                Filter::Linear => wgpu::FilterMode::Linear,
                Filter::Nearest => wgpu::FilterMode::Nearest,
            };
            let mip_filter = match sampling.mipmap {
                MipmapMode::Linear => wgpu::MipmapFilterMode::Linear,
                MipmapMode::None | MipmapMode::Nearest => wgpu::MipmapFilterMode::Nearest,
            };
            // `None` is a LOD clamp rather than a filter mode: WebGPU has no
            // "ignore the chain" switch, so pinning the max LOD to level 0 is
            // how a sampler is told to stay sharp.
            let max_lod = match sampling.mipmap {
                MipmapMode::None => 0.0,
                _ => 32.0,
            };
            self.device.create_sampler(&wgpu::SamplerDescriptor {
                label: Some("valo.image"),
                address_mode_u: address_mode(sampling.tile_x),
                address_mode_v: address_mode(sampling.tile_y),
                mag_filter: filter,
                min_filter: filter,
                mipmap_filter: mip_filter,
                lod_max_clamp: max_lod,
                ..Default::default()
            })
        })
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
        // Decal clamps at the sampler and cuts off in the shader: WebGPU has
        // no transparent border colour (`ADDRESS_MODE_CLAMP_TO_BORDER` is not
        // in the baseline), so the alternative would be a feature the web
        // target cannot have.
        TileMode::Clamp | TileMode::Decal => wgpu::AddressMode::ClampToEdge,
        TileMode::Repeat => wgpu::AddressMode::Repeat,
        TileMode::Mirror => wgpu::AddressMode::MirrorRepeat,
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
