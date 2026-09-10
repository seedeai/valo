//! Getting every decoded frame into sRGB, whatever the file declared.
//!
//! The image crate converts CICP-tagged color spaces itself; ICC profiles go through moxcms once
//! the frame is 8-bit RGBA. PNG's own gamma and chromaticity chunks are turned into a profile too,
//! so an old-style PNG is not silently relabelled as sRGB.
use crate::probe::Probe;
#[cfg(feature = "png")]
use image::metadata::Cicp;
use image::DynamicImage;
use valo_codec::{DecodeError, OpenError};

/// ColorSpace is how a frame's samples must be transformed to become sRGB.
pub(crate) enum ColorSpace {
    /// Srgb needs no transform: declared sRGB, or nothing declared.
    Srgb,
    /// Cicp is a color space the image crate converts.
    #[cfg(feature = "png")]
    Cicp(Cicp),
    /// Profile is an ICC profile applied after decoding to 8-bit RGBA.
    Profile(Box<moxcms::ColorProfile>),
}

impl ColorSpace {
    /// `detect` picks the transform from the header, PNG chunks taking precedence over ICC.
    pub(crate) fn detect(probe: &Probe, encoded: &[u8]) -> Result<Self, OpenError> {
        #[cfg(feature = "png")]
        if probe.format == image::ImageFormat::Png {
            return png_chunks::detect(encoded, probe.icc_profile.as_deref());
        }
        #[cfg(not(feature = "png"))]
        let _ = encoded;
        from_icc(probe.icc_profile.as_deref())
    }

    /// `to_srgb_rgba8` converts a decoded frame and returns its straight-alpha sRGB bytes.
    pub(crate) fn to_srgb_rgba8(&self, image: DynamicImage) -> Result<Vec<u8>, DecodeError> {
        match self {
            Self::Srgb => Ok(image.into_rgba8().into_raw()),
            #[cfg(feature = "png")]
            Self::Cicp(cicp) => {
                let mut image = image;
                image.set_color_space(*cicp).map_err(crate::decode_error)?;
                image
                    .apply_color_space(Cicp::SRGB, Default::default())
                    .map_err(crate::decode_error)?;
                Ok(image.into_rgba8().into_raw())
            }
            Self::Profile(profile) => {
                let mut rgba = image.into_rgba8().into_raw();
                apply_profile(profile, &mut rgba)?;
                Ok(rgba)
            }
        }
    }
}

fn from_icc(icc: Option<&[u8]>) -> Result<ColorSpace, OpenError> {
    match icc {
        Some(bytes) => {
            let profile = moxcms::ColorProfile::new_from_slice(bytes)
                .map_err(|error| DecodeError::InvalidData(format!("ICC profile: {error}")))?;
            Ok(ColorSpace::Profile(Box::new(profile)))
        }
        None => Ok(ColorSpace::Srgb),
    }
}

/// Converts `pixels` in place from `profile` to sRGB, RGB or gray input alike.
fn apply_profile(profile: &moxcms::ColorProfile, pixels: &mut [u8]) -> Result<(), DecodeError> {
    let target = moxcms::ColorProfile::new_srgb();
    let layout = match profile.color_space {
        moxcms::DataColorSpace::Rgb => moxcms::Layout::Rgba,
        moxcms::DataColorSpace::Gray => moxcms::Layout::GrayAlpha,
        other => {
            return Err(DecodeError::Failed(format!(
                "ICC input color space {other:?} is not supported"
            )))
        }
    };
    if layout == moxcms::Layout::Rgba {
        match profile.create_in_place_transform_8bit(layout, &target, Default::default()) {
            Ok(transform) => return transform.transform(pixels).map_err(failed),
            Err(moxcms::CmsError::UnsupportedProfileConnection) => {}
            Err(error) => return Err(failed(error)),
        }
    }
    apply_profile_blockwise(profile, layout, &target, pixels)
}

/// LUT profiles and gray-to-RGB need moxcms's separate-output transform; converting in bounded
/// blocks avoids allocating a second full frame.
fn apply_profile_blockwise(
    profile: &moxcms::ColorProfile,
    layout: moxcms::Layout,
    target: &moxcms::ColorProfile,
    pixels: &mut [u8],
) -> Result<(), DecodeError> {
    let transform = profile
        .create_transform_8bit(layout, target, moxcms::Layout::Rgba, Default::default())
        .map_err(failed)?;
    let mut output = [0u8; 4096];
    let mut gray = [0u8; 2048];
    for block in pixels.chunks_mut(output.len()) {
        if layout == moxcms::Layout::GrayAlpha {
            for (rgba, gray) in block.chunks_exact(4).zip(gray.chunks_exact_mut(2)) {
                gray[0] = rgba[0];
                gray[1] = rgba[3];
            }
            transform
                .transform(&gray[..block.len() / 2], &mut output[..block.len()])
                .map_err(failed)?;
        } else {
            transform
                .transform(block, &mut output[..block.len()])
                .map_err(failed)?;
        }
        block.copy_from_slice(&output[..block.len()]);
    }
    Ok(())
}

fn failed(error: impl std::fmt::Display) -> DecodeError {
    DecodeError::Failed(error.to_string())
}

/// PNG's color-space precedence is cICP, then iCCP, then sRGB, then cHRM/gAMA.
#[cfg(feature = "png")]
mod png_chunks {
    use super::*;
    use std::io::Cursor;

    pub(super) fn detect(encoded: &[u8], icc: Option<&[u8]>) -> Result<ColorSpace, OpenError> {
        let reader = png::Decoder::new(Cursor::new(encoded))
            .read_info()
            .map_err(|error| DecodeError::InvalidData(error.to_string()))?;
        let info = reader.info();
        if let Some(cicp) = info.coding_independent_code_points {
            return Ok(ColorSpace::Cicp(cicp_of(cicp)?));
        }
        if icc.is_some() || info.srgb.is_some() {
            return from_icc(icc);
        }
        if info.gamma().is_none() && info.chromaticities().is_none() {
            return Ok(ColorSpace::Srgb);
        }
        Ok(ColorSpace::Profile(Box::new(legacy_profile(info)?)))
    }

    fn cicp_of(cicp: png::CodingIndependentCodePoints) -> Result<Cicp, OpenError> {
        if cicp.matrix_coefficients != 0 || !cicp.is_video_full_range_image {
            return Err(OpenError::Unsupported(
                "PNG cICP with a matrix or limited range".into(),
            ));
        }
        match (cicp.color_primaries, cicp.transfer_function) {
            (1, 13) => Ok(Cicp::SRGB),
            (1, 8) => Ok(Cicp::SRGB_LINEAR),
            (12, 13) => Ok(Cicp::DISPLAY_P3),
            (primaries, transfer) => Err(OpenError::Unsupported(format!(
                "PNG cICP primaries {primaries} with transfer function {transfer}"
            ))),
        }
    }

    /// A profile built from gAMA and cHRM, the way pre-ICC PNGs declared their colour.
    fn legacy_profile(info: &png::Info) -> Result<moxcms::ColorProfile, OpenError> {
        let mut profile = moxcms::ColorProfile::new_srgb();
        profile.cicp = None;
        if let Some(gamma) = info.gamma() {
            let gamma = gamma.into_value();
            if gamma <= 0.0 {
                return Err(DecodeError::InvalidData("PNG gamma must be positive".into()).into());
            }
            let curve = moxcms::curve_from_gamma(1.0 / gamma);
            profile.red_trc = Some(curve.clone());
            profile.green_trc = Some(curve.clone());
            profile.blue_trc = Some(curve);
        }
        if let Some(chroma) = info.chromaticities() {
            let point = |(x, y): (png::ScaledFloat, png::ScaledFloat)| {
                moxcms::Chromaticity::new(x.into_value(), y.into_value())
            };
            let white = point(chroma.white);
            profile.update_rgb_colorimetry(
                moxcms::XyY::new(f64::from(white.x), f64::from(white.y), 1.0),
                moxcms::ColorPrimaries {
                    red: point(chroma.red),
                    green: point(chroma.green),
                    blue: point(chroma.blue),
                },
            );
        }
        Ok(profile)
    }
}
