//! What a caller asks of a decode, and what an opened image reports back.
use crate::DecodeError;

/// DecodeOptions says how large a decode may be and what the caller wants out of it.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct DecodeOptions {
    /// `max_size` asks for frames no larger than this box, keeping the aspect ratio.
    ///
    /// Decoders never upscale: the GPU does that for free at draw time, while decoding larger
    /// only costs memory. A native decoder scales while it decodes, which is the point of asking
    /// here rather than resizing afterwards. `None` decodes at the intrinsic size.
    pub max_size: Option<[u32; 2]>,

    /// `mipmaps` asks for a full mip chain, for images that will be drawn smaller than decoded.
    pub mipmaps: bool,

    /// `limits` bound the input and every frame, for untrusted bytes.
    pub limits: DecodeLimits,
}

impl DecodeOptions {
    /// `fit` is the output size for a source of `source` pixels under `max_size`.
    pub fn fit(&self, source: [u32; 2]) -> [u32; 2] {
        match self.max_size {
            Some(max) => fit_within(source, max),
            None => source,
        }
    }
}

/// `fit_within` scales `source` down to fit `bounds`, keeping its aspect ratio and never
/// returning a zero side. A source that already fits is returned unchanged.
pub fn fit_within(source: [u32; 2], bounds: [u32; 2]) -> [u32; 2] {
    if source[0] <= bounds[0] && source[1] <= bounds[1] {
        return source;
    }
    let by_width = [bounds[0], scaled(source[1], source[0], bounds[0])];
    if by_width[1] <= bounds[1] {
        by_width
    } else {
        [scaled(source[0], source[1], bounds[1]), bounds[1]]
    }
}

/// `value * to / from`, rounded, and at least one.
fn scaled(value: u32, from: u32, to: u32) -> u32 {
    let from = u64::from(from.max(1));
    let scaled = (u64::from(value) * u64::from(to) + from / 2) / from;
    u32::try_from(scaled).unwrap_or(u32::MAX).max(1)
}

/// DecodeLimits bounds what a decode may consume, so hostile bytes cannot exhaust memory.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct DecodeLimits {
    /// `max_encoded_bytes` bounds the input file.
    pub max_encoded_bytes: usize,

    /// `max_pixels` bounds one frame at its source size, before any downscale.
    pub max_pixels: u64,

    /// `max_frames` bounds the frame count an animation may declare.
    pub max_frames: u32,
}

impl Default for DecodeLimits {
    fn default() -> Self {
        Self {
            max_encoded_bytes: 256 * 1024 * 1024,
            max_pixels: 64 * 1024 * 1024,
            max_frames: 10_000,
        }
    }
}

impl DecodeLimits {
    /// `check_source_size` rejects an empty or too-large frame before any pixels are allocated.
    pub fn check_source_size(self, size: [u32; 2]) -> Result<(), DecodeError> {
        if size.contains(&0) {
            return Err(DecodeError::InvalidData("empty image".into()));
        }
        if u64::from(size[0]) * u64::from(size[1]) > self.max_pixels {
            return Err(DecodeError::LimitExceeded("decoded pixels"));
        }
        Ok(())
    }

    /// `check_frame_count` rejects an image with no frames or more than the limit.
    pub fn check_frame_count(self, count: u32) -> Result<(), DecodeError> {
        if count == 0 {
            return Err(DecodeError::InvalidData("image has no frames".into()));
        }
        if count > self.max_frames {
            return Err(DecodeError::LimitExceeded("animation frames"));
        }
        Ok(())
    }
}

/// ImageInfo is what is known about an image once it is open, before any frame is decoded.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct ImageInfo {
    /// `size` is the size every frame will have: the oriented source size fitted to
    /// [`DecodeOptions::max_size`].
    pub size: [u32; 2],

    /// `frame_count` is one for a still image.
    pub frame_count: u32,

    /// `repetition` is how the animation repeats after its first pass.
    pub repetition: Repetition,
}

/// Repetition is how an animation continues once it has played through.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum Repetition {
    /// Once plays a single pass, which is also what a still image does.
    #[default]
    Once,

    /// Times plays this many more passes after the first.
    Times(u32),

    /// Forever never stops, which is what most animated files ask for.
    Forever,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn fitting_keeps_aspect_and_never_upscales() {
        assert_eq!(fit_within([400, 200], [800, 800]), [400, 200]);
        assert_eq!(fit_within([400, 200], [100, 100]), [100, 50]);
        assert_eq!(fit_within([200, 400], [100, 100]), [50, 100]);
        assert_eq!(fit_within([4000, 1], [100, 100]), [100, 1]);
        assert_eq!(fit_within([3, 3], [2, 1]), [1, 1]);
    }
}
