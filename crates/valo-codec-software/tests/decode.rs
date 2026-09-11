#![cfg(all(feature = "png", feature = "gif"))]
use image::ImageEncoder;
use pollster::block_on;
use std::{sync::Arc, time::Duration};
use valo::{Color, Context, DisplayListBuilder, Paint, Rect};
use valo_codec::{DecodeError, DecodeLimits, DecodeOptions, ImageLoader, Repetition};
use valo_codec_software::SoftwareDecoder;

fn loader() -> (ImageLoader, Context) {
    let (device, queue) = valo_harness::headless_device().expect("headless GPU");
    let context = Context::new(device, queue);
    let loader = ImageLoader::new(context.image_context(), vec![Box::new(SoftwareDecoder)]);
    (loader, context)
}

fn render(context: &mut Context, image: &valo::Image) -> Vec<u8> {
    let size = image.size();
    let mut scene = DisplayListBuilder::new();
    scene.draw_image(
        image,
        Rect::new(0.0, 0.0, size[0] as f32, size[1] as f32),
        &Paint::from_color(Color::WHITE),
    );
    context.render_to_rgba(&scene.build(), size, Some(Color::TRANSPARENT))
}

fn png(profile: Option<Vec<u8>>, rotated: bool, size: [u32; 2], pixels: &[u8]) -> Arc<[u8]> {
    let mut bytes = Vec::new();
    let mut encoder = image::codecs::png::PngEncoder::new(&mut bytes);
    if let Some(profile) = profile {
        encoder.set_icc_profile(profile).unwrap();
    }
    if rotated {
        // EXIF orientation 6: rotate 90° clockwise.
        encoder
            .set_exif_metadata(vec![
                b'I', b'I', 42, 0, 8, 0, 0, 0, 1, 0, 0x12, 1, 3, 0, 1, 0, 0, 0, 6, 0, 0, 0, 0, 0,
                0, 0,
            ])
            .unwrap();
    }
    encoder
        .write_image(pixels, size[0], size[1], image::ExtendedColorType::Rgba8)
        .unwrap();
    bytes.into()
}

#[test]
fn png_is_oriented_once_and_keeps_straight_alpha() {
    let (loader, mut context) = loader();
    let bytes = png(None, true, [2, 1], &[255, 0, 0, 255, 0, 255, 0, 128]);
    let mut codec = block_on(loader.open(bytes, DecodeOptions::default())).unwrap();
    assert_eq!(codec.info().size, [1, 2]);
    let frame = block_on(codec.next_frame()).unwrap();
    assert_eq!(frame.image.size(), [1, 2]);
    assert_eq!(
        render(&mut context, &frame.image),
        [255, 0, 0, 255, 0, 255, 0, 128]
    );
}

#[test]
fn max_size_downscales_and_never_upscales() {
    let (loader, _) = loader();
    let bytes = png(None, false, [4, 2], &[200; 32]);
    let small = DecodeOptions {
        max_size: Some([2, 2]),
        ..Default::default()
    };
    assert_eq!(
        block_on(loader.decode(bytes.clone(), small))
            .unwrap()
            .size(),
        [2, 1]
    );
    let large = DecodeOptions {
        max_size: Some([100, 100]),
        ..Default::default()
    };
    assert_eq!(
        block_on(loader.decode(bytes, large)).unwrap().size(),
        [4, 2]
    );
}

#[test]
fn an_embedded_display_p3_profile_is_converted_to_srgb() {
    let (loader, mut context) = loader();
    let profile = moxcms::ColorProfile::new_display_p3().encode().unwrap();
    let bytes = png(Some(profile), false, [1, 1], &[200, 100, 40, 255]);
    let image = block_on(loader.decode(bytes, DecodeOptions::default())).unwrap();
    let actual = render(&mut context, &image);
    // Display P3 → linear sRGB matrix, then the sRGB transfer function, with 8-bit rounding.
    for (got, expected) in actual.into_iter().zip([215, 93, 6, 255]) {
        assert!(got.abs_diff(expected) <= 3, "{got} != {expected}");
    }
}

fn animated_gif() -> Arc<[u8]> {
    let mut bytes = Vec::new();
    {
        let palette = &[0, 0, 0, 255, 0, 0, 0, 255, 0, 0, 0, 255];
        let mut encoder = gif::Encoder::new(&mut bytes, 2, 1, palette).unwrap();
        encoder.set_repeat(gif::Repeat::Finite(2)).unwrap();
        for (left, width, indices, dispose) in [
            (0, 2, vec![1, 1], gif::DisposalMethod::Keep),
            (0, 1, vec![2], gif::DisposalMethod::Previous),
            (1, 1, vec![3], gif::DisposalMethod::Background),
            (0, 1, vec![2], gif::DisposalMethod::Keep),
        ] {
            let frame = gif::Frame {
                left,
                width,
                height: 1,
                delay: 7,
                dispose,
                transparent: Some(0),
                buffer: indices.into(),
                ..Default::default()
            };
            encoder.write_frame(&frame).unwrap();
        }
    }
    bytes.into()
}

#[test]
fn gif_composites_disposal_keeps_its_loop_count_and_replays_from_the_start() {
    let (loader, mut context) = loader();
    let mut codec = block_on(loader.open(animated_gif(), DecodeOptions::default())).unwrap();
    assert_eq!(codec.info().frame_count, 4);
    assert_eq!(codec.info().repetition, Repetition::Times(2));
    let frames: Vec<_> = (0..5)
        .map(|_| block_on(codec.next_frame()).unwrap())
        .collect();
    drop(codec);
    let expected = [
        [255, 0, 0, 255, 255, 0, 0, 255],
        [0, 255, 0, 255, 255, 0, 0, 255],
        [255, 0, 0, 255, 0, 0, 255, 255],
        [0, 255, 0, 255, 0, 0, 0, 0],
        [255, 0, 0, 255, 255, 0, 0, 255],
    ];
    for (frame, expected) in frames.iter().zip(expected) {
        assert_eq!(frame.duration, Duration::from_millis(70));
        assert_eq!(render(&mut context, &frame.image), expected);
    }
}

fn apng() -> Arc<[u8]> {
    let mut bytes = Vec::new();
    {
        let mut encoder = png::Encoder::new(&mut bytes, 2, 1);
        encoder.set_color(png::ColorType::Rgba);
        encoder.set_depth(png::BitDepth::Eight);
        encoder.set_animated(3, 3).unwrap();
        encoder.set_frame_delay(1, 10).unwrap();
        let mut writer = encoder.write_header().unwrap();
        writer
            .write_image_data(&[255, 0, 0, 255, 255, 0, 0, 255])
            .unwrap();
        writer.set_frame_dimension(1, 1).unwrap();
        writer.set_dispose_op(png::DisposeOp::Previous).unwrap();
        writer.write_image_data(&[0, 255, 0, 255]).unwrap();
        writer.set_frame_position(1, 0).unwrap();
        writer.set_dispose_op(png::DisposeOp::None).unwrap();
        writer.write_image_data(&[0, 0, 255, 255]).unwrap();
    }
    bytes.into()
}

#[test]
fn apng_composites_frames_and_counts_additional_passes() {
    let (loader, mut context) = loader();
    let mut codec = block_on(loader.open(apng(), DecodeOptions::default())).unwrap();
    assert_eq!(codec.info().frame_count, 3);
    assert_eq!(codec.info().repetition, Repetition::Times(2));
    for expected in [
        [255, 0, 0, 255, 255, 0, 0, 255],
        [0, 255, 0, 255, 255, 0, 0, 255],
        [255, 0, 0, 255, 0, 0, 255, 255],
    ] {
        let frame = block_on(codec.next_frame()).unwrap();
        assert_eq!(frame.duration, Duration::from_millis(100));
        assert_eq!(render(&mut context, &frame.image), expected);
    }
}

#[test]
fn a_damaged_frame_is_an_error_and_the_frame_limit_bounds_header_scanning() {
    let (loader, _) = loader();
    let mut bytes = apng().to_vec();
    let position = bytes.windows(4).position(|chunk| chunk == b"fdAT").unwrap();
    bytes[position + 8] ^= 255;
    let mut codec = block_on(loader.open(bytes.into(), DecodeOptions::default())).unwrap();
    assert!(block_on(codec.next_frame()).is_ok());
    assert!(block_on(codec.next_frame()).is_err());

    let two_frames = DecodeOptions {
        limits: DecodeLimits {
            max_frames: 2,
            ..Default::default()
        },
        ..Default::default()
    };
    assert!(matches!(
        block_on(loader.open(animated_gif(), two_frames)),
        Err(DecodeError::LimitExceeded("animation frames"))
    ));
}

#[test]
fn png_gamma_and_cicp_are_converted_rather_than_relabelled() {
    let (loader, mut context) = loader();
    for cicp in [false, true] {
        let mut bytes = Vec::new();
        {
            let mut info = png::Info::with_size(1, 1);
            info.color_type = png::ColorType::Rgba;
            info.bit_depth = png::BitDepth::Eight;
            if !cicp {
                info.source_gamma = Some(png::ScaledFloat::new(1.0));
            }
            let mut writer = png::Encoder::with_info(&mut bytes, info)
                .unwrap()
                .write_header()
                .unwrap();
            // Linear sRGB, written by hand: the png encoder does not emit cICP itself.
            if cicp {
                writer.write_chunk(png::chunk::cICP, &[1, 8, 0, 1]).unwrap();
            }
            writer.write_image_data(&[128, 128, 128, 255]).unwrap();
        }
        let image = block_on(loader.decode(bytes.into(), DecodeOptions::default())).unwrap();
        let pixels = render(&mut context, &image);
        for value in &pixels[..3] {
            assert!(value.abs_diff(188) <= 2, "cicp={cicp}, {pixels:?}");
        }
    }
}

#[test]
fn a_grayscale_icc_profile_converts_to_rgb_and_keeps_alpha() {
    let (loader, mut context) = loader();
    let mut bytes = Vec::new();
    let mut encoder = image::codecs::png::PngEncoder::new(&mut bytes);
    encoder
        .set_icc_profile(
            moxcms::ColorProfile::new_gray_with_gamma(1.0)
                .encode()
                .unwrap(),
        )
        .unwrap();
    encoder
        .write_image(&[128, 128], 1, 1, image::ExtendedColorType::La8)
        .unwrap();
    let image = block_on(loader.decode(bytes.into(), DecodeOptions::default())).unwrap();
    let pixels = render(&mut context, &image);
    for value in &pixels[..3] {
        assert!(value.abs_diff(188) <= 2, "{pixels:?}");
    }
    assert_eq!(pixels[3], 128);
}

#[test]
fn unrecognised_bytes_are_declined_so_another_decoder_could_take_them() {
    let (loader, _) = loader();
    match block_on(loader.decode(Arc::from(*b"not an image at all"), DecodeOptions::default())) {
        Err(DecodeError::Unsupported(declined)) => assert_eq!(declined[0].decoder, "software"),
        other => panic!("{other:?}"),
    }
}
