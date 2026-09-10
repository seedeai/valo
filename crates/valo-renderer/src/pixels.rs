//! CPU pixel descriptions accepted at the upload boundary.

/// PixelFormat is the channel order of four eight-bit image components.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum PixelFormat {
    /// Rgba8 stores red, green, blue and alpha in that order.
    Rgba8,

    /// Bgra8 stores blue, green, red and alpha in that order.
    Bgra8,
}

/// AlphaType describes whether a pixel's color components include its alpha.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum AlphaType {
    /// Premultiplied stores color multiplied by alpha.
    Premultiplied,

    /// Straight stores color independently of alpha.
    Straight,
}

/// PixelLayout describes top-to-bottom rows of sRGB image samples.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct PixelLayout {
    /// `size` is the image width and height in pixels.
    pub size: [u32; 2],

    /// `row_bytes` includes any padding between adjacent rows.
    pub row_bytes: u32,

    /// `format` is the channel order within each pixel.
    pub format: PixelFormat,

    /// `alpha` describes the color/alpha relationship.
    pub alpha: AlphaType,
}

impl PixelLayout {
    /// `packed` describes rows with no padding in the given format and alpha.
    pub fn packed(size: [u32; 2], format: PixelFormat, alpha: AlphaType) -> Self {
        Self {
            size,
            row_bytes: size[0].saturating_mul(4),
            format,
            alpha,
        }
    }

    /// `byte_len` checks dimensions and row stride before computing storage size.
    pub fn byte_len(self) -> Result<usize, ImageError> {
        let width = self.size[0]
            .checked_mul(4)
            .ok_or(ImageError::InvalidLayout)?;
        if self.size.contains(&0) || self.row_bytes < width {
            return Err(ImageError::InvalidLayout);
        }
        usize::try_from(u64::from(self.row_bytes) * u64::from(self.size[1]))
            .map_err(|_| ImageError::InvalidLayout)
    }
}

/// PixelBuffer owns decoded CPU samples and their row layout.
///
/// Uploading consumes the allocation, normalizing channels and alpha in place before transfer,
/// so a decoder hands over whatever layout it produced without an extra copy.
pub struct PixelBuffer {
    layout: PixelLayout,
    pixels: Vec<u8>,
}

impl PixelBuffer {
    /// `new` takes ownership of an existing CPU allocation without copying its contents.
    pub fn new(layout: PixelLayout, pixels: Vec<u8>) -> Result<Self, ImageError> {
        if pixels.len() != layout.byte_len()? {
            return Err(ImageError::InvalidLayout);
        }
        Ok(Self { layout, pixels })
    }

    /// `layout` returns the dimensions and stride the samples follow.
    pub fn layout(&self) -> PixelLayout {
        self.layout
    }

    /// `into_premultiplied_rgba` compacts rows and converts channels and alpha in place.
    pub(crate) fn into_premultiplied_rgba(self) -> Vec<u8> {
        let Self { layout, mut pixels } = self;
        let row = layout.size[0] as usize * 4;
        for y in 0..layout.size[1] as usize {
            let from = y * layout.row_bytes as usize;
            pixels.copy_within(from..from + row, y * row);
            normalize_row(&mut pixels[y * row..(y + 1) * row], layout);
        }
        pixels.truncate(row * layout.size[1] as usize);
        pixels
    }
}

fn normalize_row(row: &mut [u8], layout: PixelLayout) {
    for pixel in row.chunks_exact_mut(4) {
        if layout.format == PixelFormat::Bgra8 {
            pixel.swap(0, 2);
        }
        if layout.alpha == AlphaType::Straight {
            premultiply(pixel);
        }
    }
}

fn premultiply(pixel: &mut [u8]) {
    let alpha = u16::from(pixel[3]);
    for channel in &mut pixel[..3] {
        *channel = ((u16::from(*channel) * alpha + 127) / 255) as u8;
    }
}

/// ImageError reports an invalid image layout or a texture the renderer cannot sample.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum ImageError {
    /// InvalidLayout means dimensions, stride or buffer length do not describe an image.
    InvalidLayout,

    /// TooLarge means the image exceeds this device's texture limits.
    TooLarge,

    /// IncompatibleTexture means the texture cannot be sampled as a premultiplied sRGB image.
    IncompatibleTexture,

    /// WrongDevice means a native texture belongs to another GPU device.
    WrongDevice,

    /// UnsupportedBackend means this wgpu backend cannot import the native resource.
    UnsupportedBackend,
}

impl std::fmt::Display for ImageError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{self:?}")
    }
}

impl std::error::Error for ImageError {}
