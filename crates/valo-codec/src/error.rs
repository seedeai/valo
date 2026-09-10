//! Why a decode did not produce an image.
use std::fmt;
use valo::ImageError;

/// DecodeError is why a decode did not produce an image.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum DecodeError {
    /// NoDecoder means the loader was built with no decoders at all.
    NoDecoder,

    /// Unsupported means every decoder declined the bytes; each entry says why.
    Unsupported(Vec<Declined>),

    /// InvalidData means a decoder recognised the format and found the bytes damaged.
    InvalidData(String),

    /// LimitExceeded means a [`DecodeLimits`](crate::DecodeLimits) bound was hit; the text names
    /// which one.
    LimitExceeded(&'static str),

    /// Failed means a decoder or the platform failed without proving the bytes invalid — an
    /// allocation failure, a system API declining.
    Failed(String),

    /// Image means the decoded frame could not become an image on this device.
    Image(ImageError),

    /// Closed means the loader was dropped before the work ran.
    Closed,
}

/// Declined records one decoder passing on an image.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Declined {
    /// `decoder` is the [`Decoder::name`](crate::Decoder::name).
    pub decoder: &'static str,

    /// `reason` is that decoder's own account.
    pub reason: String,
}

impl From<ImageError> for DecodeError {
    fn from(error: ImageError) -> Self {
        Self::Image(error)
    }
}

impl fmt::Display for DecodeError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::NoDecoder => f.write_str("no image decoder is registered"),
            Self::Unsupported(declined) => {
                f.write_str("no registered decoder reads this image")?;
                for entry in declined {
                    write!(f, "; {}: {}", entry.decoder, entry.reason)?;
                }
                Ok(())
            }
            Self::InvalidData(reason) => write!(f, "damaged image: {reason}"),
            Self::LimitExceeded(limit) => write!(f, "decode limit exceeded: {limit}"),
            Self::Failed(reason) => write!(f, "decode failed: {reason}"),
            Self::Image(error) => write!(f, "image creation failed: {error}"),
            Self::Closed => f.write_str("the image loader was closed before decoding"),
        }
    }
}

impl std::error::Error for DecodeError {}
