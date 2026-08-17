use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::Arc;

/// `Image` is a cheap-to-clone handle to an immutable GPU image.
///
/// Display lists retain cloned handles, and the texture is released after the
/// last handle and in-flight GPU use are gone.
#[derive(Clone)]
pub struct Image {
    inner: Arc<ImageInner>,
}

/// `ImageInner` contains the shared resources and metadata behind an [`Image`].
pub struct ImageInner {
    /// `id` is the process-unique identity of this image.
    pub id: u64,
    /// `size` is the image dimensions in pixels.
    pub size: [u32; 2],
    /// `texture` stores the image pixels.
    pub texture: wgpu::Texture,
    /// `view` exposes the complete texture for sampling.
    pub view: wgpu::TextureView,
    /// `mip_levels` is the number of available mip levels.
    pub mip_levels: u32,
}

/// `Image` equality compares image identity rather than pixel contents.
impl PartialEq for Image {
    fn eq(&self, other: &Self) -> bool {
        self.inner.id == other.inner.id
    }
}

static NEXT_IMAGE_ID: AtomicU64 = AtomicU64::new(1);

impl Image {
    /// `from_texture` creates an image from an existing texture.
    ///
    /// Prefer `Context::import_image` through the `valo` facade unless managing
    /// renderer resources directly.
    pub fn from_texture(texture: wgpu::Texture, size: [u32; 2], mip_levels: u32) -> Self {
        let view = texture.create_view(&wgpu::TextureViewDescriptor::default());
        Self {
            inner: Arc::new(ImageInner {
                id: NEXT_IMAGE_ID.fetch_add(1, Ordering::Relaxed),
                size,
                texture,
                view,
                mip_levels,
            }),
        }
    }

    /// `id` returns the process-unique identity of this image.
    pub fn id(&self) -> u64 {
        self.inner.id
    }

    /// `size` returns the image dimensions in pixels.
    pub fn size(&self) -> [u32; 2] {
        self.inner.size
    }

    /// `width` returns the image width in pixels.
    pub fn width(&self) -> f32 {
        self.inner.size[0] as f32
    }

    /// `height` returns the image height in pixels.
    pub fn height(&self) -> f32 {
        self.inner.size[1] as f32
    }

    /// `view` returns the texture view used for sampling.
    pub fn view(&self) -> &wgpu::TextureView {
        &self.inner.view
    }

    /// `texture` returns the underlying GPU texture.
    pub fn texture(&self) -> &wgpu::Texture {
        &self.inner.texture
    }

    /// `mip_levels` returns the number of available mip levels.
    pub fn mip_levels(&self) -> u32 {
        self.inner.mip_levels
    }

    /// `downgrade` returns a non-owning handle to this image's resources.
    pub fn downgrade(&self) -> std::sync::Weak<ImageInner> {
        Arc::downgrade(&self.inner)
    }
}

impl std::fmt::Debug for Image {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Image")
            .field("id", &self.inner.id)
            .field("size", &self.inner.size)
            .field("mip_levels", &self.inner.mip_levels)
            .finish()
    }
}

#[cfg(feature = "serde")]
impl serde::Serialize for Image {
    /// Dumps identity, not pixels: the serde dump exists for diffs and bug
    /// reports, never for persisting GPU state.
    fn serialize<S: serde::Serializer>(&self, s: S) -> Result<S::Ok, S::Error> {
        use serde::ser::SerializeStruct;
        let mut st = s.serialize_struct("Image", 2)?;
        st.serialize_field("id", &self.inner.id)?;
        st.serialize_field("size", &self.inner.size)?;
        st.end()
    }
}

/// `Sampling` controls filtering, mip selection, and behavior outside an image.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Hash)]
#[cfg_attr(feature = "serde", derive(serde::Serialize, serde::Deserialize))]
pub struct Sampling {
    /// `filter` controls interpolation between neighboring texels.
    pub filter: Filter,
    /// `mipmap` controls level selection when mipmaps are available.
    pub mipmap: MipmapMode,
    /// `tile_x` controls sampling outside the horizontal image bounds.
    pub tile_x: TileMode,
    /// `tile_y` controls sampling outside the vertical image bounds.
    pub tile_y: TileMode,
}

/// `MipmapMode` controls how minified images select mip levels.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Hash)]
#[cfg_attr(feature = "serde", derive(serde::Serialize, serde::Deserialize))]
pub enum MipmapMode {
    /// `None` always samples level zero.
    None,
    /// `Nearest` samples the closest mip level.
    Nearest,
    /// `Linear` blends the two nearest mip levels.
    #[default]
    Linear,
}

/// `Filter` controls interpolation between neighboring texels.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Hash)]
#[cfg_attr(feature = "serde", derive(serde::Serialize, serde::Deserialize))]
pub enum Filter {
    /// `Linear` uses bilinear interpolation.
    #[default]
    Linear,
    /// `Nearest` selects the nearest texel without interpolation.
    Nearest,
}

/// `TileMode` controls samples outside an image's bounds.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Hash)]
#[cfg_attr(feature = "serde", derive(serde::Serialize, serde::Deserialize))]
pub enum TileMode {
    /// `Clamp` extends the nearest edge texel.
    #[default]
    Clamp,
    /// `Repeat` repeats the image in the same orientation.
    Repeat,
    /// `Mirror` repeats the image with alternating orientation.
    Mirror,
    /// `Decal` returns transparent pixels outside the image.
    Decal,
}
