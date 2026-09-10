//! GPU image creation shared by rendering and decoding, independent of drawing caches.
use crate::images::{ImageDesc, IMAGE_FORMAT};
use crate::mips::{full_mip_count, MipGenerator};
use crate::pixels::{ImageError, PixelBuffer};
use std::sync::Arc;
use valo_dl::Image;

/// ImageContext turns pixels or textures into drawable images on the renderer's device.
///
/// It is the piece of the renderer an image decoder needs: the device, the queue and the mip
/// pipeline, and nothing that a frame in flight mutates. Obtain one from
/// `Context::image_context`; clones share the pipeline and may be used from any thread, so a
/// decode worker can upload its result and hand back a finished [`Image`].
#[derive(Clone)]
pub struct ImageContext {
    device: wgpu::Device,
    queue: wgpu::Queue,
    mips: Arc<MipGenerator>,
}

impl ImageContext {
    /// `new` creates image operations over a host-owned device and queue.
    pub fn new(device: wgpu::Device, queue: wgpu::Queue) -> Self {
        let mips = Arc::new(MipGenerator::new(&device));
        Self {
            device,
            queue,
            mips,
        }
    }

    /// `device` is the device every image made here belongs to.
    pub fn device(&self) -> &wgpu::Device {
        &self.device
    }

    /// `queue` is the queue image preparation commands are submitted on.
    pub fn queue(&self) -> &wgpu::Queue {
        &self.queue
    }

    /// `check_size` rejects empty images and dimensions exceeding the device's limits.
    pub fn check_size(&self, size: [u32; 2]) -> Result<(), ImageError> {
        if size.contains(&0) {
            return Err(ImageError::InvalidLayout);
        }
        let limit = self.device.limits().max_texture_dimension_2d;
        if size.into_iter().any(|side| side > limit) {
            return Err(ImageError::TooLarge);
        }
        Ok(())
    }

    /// `upload_pixels` consumes CPU samples and uploads a drawable image.
    ///
    /// Channel order and alpha are normalized in the consumed allocation, so any layout a
    /// decoder produced is accepted without a second buffer.
    pub fn upload_pixels(&self, pixels: PixelBuffer, mipmaps: bool) -> Result<Image, ImageError> {
        let layout = pixels.layout();
        self.check_size(layout.size)?;
        let rgba = pixels.into_premultiplied_rgba();
        Ok(self.upload_rgba(
            ImageDesc {
                size: layout.size,
                premultiplied: true,
                mips: mipmaps,
            },
            &rgba,
        ))
    }

    /// `import_texture` retains a texture that already holds premultiplied sRGB samples.
    ///
    /// The texture must be a single-sample 2D `Rgba8Unorm` or `Bgra8Unorm` texture with
    /// `TEXTURE_BINDING` usage on this device. When `mipmaps` is set and the texture lacks a full
    /// chain, level 0 is copied on the GPU into a new texture with the chain generated; the
    /// original backing is never read back.
    pub fn import_texture(
        &self,
        texture: wgpu::Texture,
        mipmaps: bool,
    ) -> Result<Image, ImageError> {
        check_sampleable(&texture)?;
        let size = [texture.width(), texture.height()];
        self.check_size(size)?;
        #[cfg(feature = "trace")]
        tracing::trace!(operation = "import", "image transfer");
        let levels = texture.mip_level_count().max(1);
        let image = Image::from_texture(texture, size, levels);
        Ok(self.with_requested_mips(image, mipmaps))
    }

    /// `upload_rgba` creates a retained [`Image`] from RGBA8 pixels.
    ///
    /// Premultiplies when `desc.premultiplied` is false, writes mip level 0,
    /// and builds the mip chain when `desc.mips` is true. Panics if
    /// `pixels.len()` is not `width * height * 4`.
    pub(crate) fn upload_rgba(&self, desc: ImageDesc, pixels: &[u8]) -> Image {
        let [w, h] = desc.size;
        assert_eq!(pixels.len(), (w * h * 4) as usize, "RGBA8 pixel count");
        let premultiplied = premultiplied_pixels(desc.premultiplied, pixels);
        let mip_levels = if desc.mips {
            full_mip_count(desc.size)
        } else {
            1
        };
        let texture = self.create_image_texture(desc.size, mip_levels);
        self.write_level_zero(&texture, desc.size, &premultiplied);
        self.finish_external(texture, desc.size, mip_levels)
    }

    /// `finish_external` wraps an already-populated texture as a retained [`Image`].
    ///
    /// Use this when the host copied pixels itself (for example an
    /// `ImageBitmap` upload). Builds the mip chain when `mip_levels` is
    /// greater than 1.
    pub(crate) fn finish_external(
        &self,
        texture: wgpu::Texture,
        size: [u32; 2],
        mip_levels: u32,
    ) -> Image {
        if mip_levels > 1 {
            self.mips
                .generate(&self.device, &self.queue, &texture, mip_levels);
        }
        Image::from_texture(texture, size, mip_levels)
    }

    /// `regenerate_mips` rebuilds the mip chain after level 0 was rewritten in place.
    ///
    /// Call this after each copy from a per-frame source such as a video frame.
    pub(crate) fn regenerate_mips(&self, image: &Image) {
        if image.mip_levels() > 1 {
            self.mips.generate(
                &self.device,
                &self.queue,
                image.texture(),
                image.mip_levels(),
            );
        }
    }

    /// `create_image_texture` allocates an empty image texture of `size` and `mip_levels`.
    ///
    /// The texture is bindable, copy-destination, and a render attachment so
    /// mip levels can be generated by rendering into them.
    pub(crate) fn create_image_texture(&self, size: [u32; 2], mip_levels: u32) -> wgpu::Texture {
        self.device.create_texture(&wgpu::TextureDescriptor {
            label: Some("valo.image"),
            size: extent(size),
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

    /// Gives an imported image a full RGBA mip chain when asked for and not already present.
    fn with_requested_mips(&self, image: Image, mipmaps: bool) -> Image {
        let levels = if mipmaps {
            full_mip_count(image.size())
        } else {
            1
        };
        if image.mip_levels() >= levels {
            return image;
        }
        #[cfg(feature = "trace")]
        tracing::trace!(operation = "copy_for_mipmaps", "image transfer");
        let texture = self.create_image_texture(image.size(), levels);
        self.mips
            .copy_level_zero(&self.device, &self.queue, image.texture(), &texture);
        self.finish_external(texture, image.size(), levels)
    }

    fn write_level_zero(&self, texture: &wgpu::Texture, size: [u32; 2], premultiplied: &[u8]) {
        #[cfg(feature = "trace")]
        tracing::trace!(operation = "upload", "image transfer");
        self.queue.write_texture(
            wgpu::TexelCopyTextureInfo {
                texture,
                mip_level: 0,
                origin: wgpu::Origin3d::ZERO,
                aspect: wgpu::TextureAspect::All,
            },
            premultiplied,
            wgpu::TexelCopyBufferLayout {
                offset: 0,
                bytes_per_row: Some(size[0] * 4),
                rows_per_image: None,
            },
            extent(size),
        );
    }
}

fn check_sampleable(texture: &wgpu::Texture) -> Result<(), ImageError> {
    let shape_ok = texture.dimension() == wgpu::TextureDimension::D2
        && texture.depth_or_array_layers() == 1
        && texture.sample_count() == 1;
    let usage_ok = texture
        .usage()
        .contains(wgpu::TextureUsages::TEXTURE_BINDING);
    let format_ok = matches!(
        texture.format(),
        wgpu::TextureFormat::Rgba8Unorm | wgpu::TextureFormat::Bgra8Unorm
    );
    if shape_ok && usage_ok && format_ok {
        Ok(())
    } else {
        Err(ImageError::IncompatibleTexture)
    }
}

fn extent(size: [u32; 2]) -> wgpu::Extent3d {
    wgpu::Extent3d {
        width: size[0],
        height: size[1],
        depth_or_array_layers: 1,
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
