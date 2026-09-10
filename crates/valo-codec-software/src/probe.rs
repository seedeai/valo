//! Reading an image's header: format, size, orientation, and how many frames it holds.
use crate::decode_error;
use image::metadata::Orientation;
use image::{ImageDecoder, ImageFormat, ImageReader};
use std::io::Cursor;
use valo_codec::{DecodeError, DecodeLimits, OpenError, Repetition};

/// Probe is what the header says about an image, before any pixels are decoded.
pub(crate) struct Probe {
    pub format: ImageFormat,
    pub orientation: Orientation,
    /// The size after `orientation` is applied, which is the size frames come out at.
    pub oriented_size: [u32; 2],
    pub frame_count: u32,
    pub repetition: Repetition,
    /// Whether frames come from the animation path; a one-frame animation still keeps its timing.
    #[cfg_attr(not(animation), allow(dead_code))]
    pub animated: bool,
    pub icc_profile: Option<Vec<u8>>,
}

/// `probe` reads the header, declining formats this build does not read.
pub(crate) fn probe(encoded: &[u8], limits: DecodeLimits) -> Result<Probe, OpenError> {
    let reader = ImageReader::new(Cursor::new(encoded))
        .with_guessed_format()
        .map_err(|error| DecodeError::Failed(error.to_string()))?;
    let format = reader
        .format()
        .ok_or_else(|| OpenError::Unsupported("unrecognised image format".into()))?;
    let mut decoder = reader.into_decoder().map_err(open_error)?;
    let (width, height) = decoder.dimensions();
    limits.check_source_size([width, height])?;
    let orientation = decoder.orientation().map_err(decode_error)?;
    let icc_profile = decoder.icc_profile().map_err(decode_error)?;
    let animation = animation_info(encoded, format, limits)?;
    Ok(Probe {
        format,
        orientation,
        oriented_size: oriented([width, height], orientation),
        frame_count: animation.frame_count,
        repetition: animation.repetition,
        animated: animation.animated,
        icc_profile,
    })
}

/// A format the image crate recognises but this build did not compile in is a decline, not a
/// failure: another decoder may read it.
fn open_error(error: image::ImageError) -> OpenError {
    match error {
        image::ImageError::Unsupported(error) => OpenError::Unsupported(error.to_string()),
        other => OpenError::Failed(decode_error(other)),
    }
}

fn oriented(size: [u32; 2], orientation: Orientation) -> [u32; 2] {
    match orientation {
        Orientation::Rotate90
        | Orientation::Rotate270
        | Orientation::Rotate90FlipH
        | Orientation::Rotate270FlipH => [size[1], size[0]],
        _ => size,
    }
}

struct AnimationInfo {
    frame_count: u32,
    repetition: Repetition,
    animated: bool,
}

const STILL: AnimationInfo = AnimationInfo {
    frame_count: 1,
    repetition: Repetition::Once,
    animated: false,
};

/// Container-level animation metadata, read with each format's own parser because the image
/// crate exposes neither loop counts nor frame counts without decoding every frame.
fn animation_info(
    encoded: &[u8],
    format: ImageFormat,
    limits: DecodeLimits,
) -> Result<AnimationInfo, DecodeError> {
    #[cfg(not(animation))]
    let _ = (encoded, limits);
    match format {
        #[cfg(feature = "gif")]
        ImageFormat::Gif => gif_info(encoded, limits),
        #[cfg(feature = "png")]
        ImageFormat::Png => png_info(encoded, limits),
        #[cfg(feature = "webp")]
        ImageFormat::WebP => webp_info(encoded, limits),
        _ => Ok(STILL),
    }
}

/// A file that loops `n` times after the first pass; zero passes after means play once.
#[cfg(animation)]
fn repeats(additional_passes: u32) -> Repetition {
    match additional_passes {
        0 => Repetition::Once,
        n => Repetition::Times(n),
    }
}

#[cfg(animation)]
fn invalid(error: impl std::fmt::Display) -> DecodeError {
    DecodeError::InvalidData(error.to_string())
}

#[cfg(feature = "gif")]
fn gif_info(encoded: &[u8], limits: DecodeLimits) -> Result<AnimationInfo, DecodeError> {
    let mut options = gif::DecodeOptions::new();
    options.skip_frame_decoding(true);
    options.check_frame_consistency(true);
    let mut decoder = options.read_info(Cursor::new(encoded)).map_err(invalid)?;
    let mut frame_count = 0;
    while decoder.next_frame_info().map_err(invalid)?.is_some() {
        frame_count += 1;
        if frame_count > limits.max_frames {
            return Err(DecodeError::LimitExceeded("animation frames"));
        }
    }
    let repetition = match decoder.repeat() {
        gif::Repeat::Infinite => Repetition::Forever,
        gif::Repeat::Finite(passes) => repeats(u32::from(passes)),
    };
    Ok(AnimationInfo {
        frame_count,
        repetition,
        animated: true,
    })
}

#[cfg(feature = "png")]
fn png_info(encoded: &[u8], limits: DecodeLimits) -> Result<AnimationInfo, DecodeError> {
    let reader = png::Decoder::new(Cursor::new(encoded))
        .read_info()
        .map_err(invalid)?;
    let Some(control) = reader.info().animation_control else {
        return Ok(STILL);
    };
    limits.check_frame_count(control.num_frames)?;
    let repetition = match control.num_plays {
        0 => Repetition::Forever,
        plays => repeats(plays - 1),
    };
    Ok(AnimationInfo {
        frame_count: control.num_frames,
        repetition,
        animated: true,
    })
}

#[cfg(feature = "webp")]
fn webp_info(encoded: &[u8], limits: DecodeLimits) -> Result<AnimationInfo, DecodeError> {
    let decoder = image_webp::WebPDecoder::new(Cursor::new(encoded)).map_err(invalid)?;
    if !decoder.is_animated() {
        return Ok(STILL);
    }
    let frame_count = decoder.num_frames().max(1);
    limits.check_frame_count(frame_count)?;
    let repetition = match decoder.loop_count() {
        image_webp::LoopCount::Forever => Repetition::Forever,
        image_webp::LoopCount::Times(plays) => repeats(u32::from(plays.get()) - 1),
    };
    Ok(AnimationInfo {
        frame_count,
        repetition,
        animated: true,
    })
}
