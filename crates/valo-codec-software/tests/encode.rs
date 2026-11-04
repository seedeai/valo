#![cfg(all(feature = "png", feature = "jpeg", feature = "webp"))]
//! What `encode` writes, read back with the `image` crate's decoders.

use valo::{AlphaType, Color, PixelBuffer, PixelFormat, PixelLayout};
use valo_codec_software::{encode, EncodeError, Encoding};

/// `pixels` is a 2×2 picture: opaque red, half-transparent green, transparent, opaque blue.
fn pixels() -> PixelBuffer {
    let layout = PixelLayout::packed([2, 2], PixelFormat::Rgba8, AlphaType::Straight);
    let rgba = vec![255, 0, 0, 255, 0, 255, 0, 128, 0, 0, 0, 0, 0, 0, 255, 255];
    PixelBuffer::new(layout, rgba).unwrap()
}

fn decoded(bytes: &[u8]) -> Vec<u8> {
    image::load_from_memory(bytes)
        .unwrap()
        .to_rgba8()
        .into_raw()
}

#[test]
fn png_and_lossless_webp_keep_every_pixel() {
    for encoding in [Encoding::Png, Encoding::WebpLossless] {
        let bytes = encode(&pixels(), encoding).unwrap();
        assert_eq!(decoded(&bytes), pixels().bytes(), "{encoding:?}");
    }
}

#[test]
fn a_jpeg_lies_over_its_background() {
    let white = Encoding::Jpeg {
        quality: 100,
        background: Color::WHITE,
    };
    let bytes = encode(&pixels(), white).unwrap();
    assert_eq!(&bytes[..2], [0xFF, 0xD8], "a JPEG's start of image");
    let rgba = decoded(&bytes);
    let transparent = &rgba[8..11];
    assert!(
        transparent.iter().all(|&channel| channel > 240),
        "the transparent pixel shows white, not {transparent:?}"
    );
}

#[test]
fn a_jpeg_quality_outside_1_to_100_is_refused() {
    let jpeg = |quality| Encoding::Jpeg {
        quality,
        background: Color::WHITE,
    };
    assert_eq!(encode(&pixels(), jpeg(0)), Err(EncodeError::Quality(0)));
    assert_eq!(encode(&pixels(), jpeg(101)), Err(EncodeError::Quality(101)));
}

#[test]
fn padded_bgra_premultiplied_rows_encode_as_their_straight_rgba() {
    let layout = PixelLayout {
        size: [2, 2],
        row_bytes: 12,
        format: PixelFormat::Bgra8,
        alpha: AlphaType::Premultiplied,
    };
    let bgra = vec![
        0, 0, 255, 255, 0, 128, 0, 128, 9, 9, 9, 9, // row 1, then 4 bytes of padding
        0, 0, 0, 0, 255, 0, 0, 255, 9, 9, 9, 9, // row 2
    ];
    let bytes = encode(&PixelBuffer::new(layout, bgra).unwrap(), Encoding::Png).unwrap();
    assert_eq!(decoded(&bytes), pixels().bytes());
}
