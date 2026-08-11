use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::Arc;

/// An uploaded, immutable image. A cheap-clone `Arc` handle: hosts keep it,
/// display lists reference it, and DROP is the whole lifetime story — wgpu
/// keeps the texture alive until in-flight submissions finish.
///
/// Lives in valo-dl so ops can hold it directly (Flutter's DlImage does the
/// same). Recording stays device-free — you just can't MINT one without a
/// `Context` (`upload_image`), which is the point.
#[derive(Clone)]
pub struct Image {
    inner: Arc<ImageInner>,
}

pub struct ImageInner {
    pub id: u64,
    pub size: [u32; 2],
    pub texture: wgpu::Texture,
    pub view: wgpu::TextureView,
    pub mip_levels: u32,
}

/// Two handles are the same image when they name the same upload — the id is
/// process-unique, so this never compares texture contents.
impl PartialEq for Image {
    fn eq(&self, other: &Self) -> bool {
        self.inner.id == other.inner.id
    }
}

static NEXT_IMAGE_ID: AtomicU64 = AtomicU64::new(1);

impl Image {
    /// Called by the renderer's upload path — hosts never construct these.
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

    pub fn id(&self) -> u64 {
        self.inner.id
    }

    pub fn size(&self) -> [u32; 2] {
        self.inner.size
    }

    pub fn width(&self) -> f32 {
        self.inner.size[0] as f32
    }

    pub fn height(&self) -> f32 {
        self.inner.size[1] as f32
    }

    pub fn view(&self) -> &wgpu::TextureView {
        &self.inner.view
    }

    pub fn texture(&self) -> &wgpu::Texture {
        &self.inner.texture
    }

    pub fn mip_levels(&self) -> u32 {
        self.inner.mip_levels
    }

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

/// How image draws sample their texture.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Hash)]
#[cfg_attr(feature = "serde", derive(serde::Serialize, serde::Deserialize))]
pub struct Sampling {
    pub filter: Filter,
    pub tile_x: TileMode,
    pub tile_y: TileMode,
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Hash)]
#[cfg_attr(feature = "serde", derive(serde::Serialize, serde::Deserialize))]
pub enum Filter {
    /// Bilinear + mip selection when the image has mips (downscaled artwork).
    #[default]
    Linear,
    /// Pixel-art / QR-code mode.
    Nearest,
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Hash)]
#[cfg_attr(feature = "serde", derive(serde::Serialize, serde::Deserialize))]
pub enum TileMode {
    #[default]
    Clamp,
    Repeat,
    Mirror,
}
