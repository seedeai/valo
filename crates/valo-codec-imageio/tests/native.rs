#![cfg(any(target_os = "macos", target_os = "ios"))]
use image::ImageEncoder;
use pollster::block_on;
use std::{io::Cursor, sync::Arc, time::Duration};
use valo::{Color, Context, DisplayListBuilder, Paint, Rect};
use valo_codec::{DecodeError, DecodeOptions, Decoder, ImageLoader, Repetition};
use valo_codec_imageio::ImageIoDecoder;
use valo_codec_software::SoftwareDecoder;

fn setup(decoders: Vec<Box<dyn Decoder>>) -> (ImageLoader, Context) {
    let (device, queue) = valo_harness::headless_device().expect("GPU required");
    let context = Context::new(device, queue);
    (ImageLoader::new(context.image_context(), decoders), context)
}

fn imageio(prefer_shared: bool) -> Box<dyn Decoder> {
    Box::new(ImageIoDecoder { prefer_shared })
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

const PIXELS_3X2: [u8; 24] = [
    255, 0, 0, 255, 0, 255, 0, 255, 0, 0, 255, 255, 255, 255, 255, 128, 255, 255, 0, 255, 0, 0, 0,
    0,
];

fn png_3x2() -> Arc<[u8]> {
    let pixels = image::RgbaImage::from_raw(3, 2, PIXELS_3X2.to_vec()).unwrap();
    let mut bytes = Cursor::new(Vec::new());
    image::DynamicImage::ImageRgba8(pixels)
        .write_to(&mut bytes, image::ImageFormat::Png)
        .unwrap();
    bytes.into_inner().into()
}

#[test]
fn shared_bgra_and_uploaded_rgba_paths_draw_the_same_pixels() {
    let mut results = Vec::new();
    for shared in [false, true] {
        let (loader, mut context) = setup(vec![imageio(shared)]);
        let image = block_on(loader.decode(png_3x2(), DecodeOptions::default())).unwrap();
        assert_eq!(
            image.texture().format(),
            if shared {
                wgpu::TextureFormat::Bgra8Unorm
            } else {
                wgpu::TextureFormat::Rgba8Unorm
            }
        );
        assert_eq!(image.size(), [3, 2]);
        results.push(render(&mut context, &image));
    }
    assert_eq!(results[0], results[1]);
    for (got, expected) in results[0].iter().zip(PIXELS_3X2) {
        assert!(got.abs_diff(expected) <= 1, "{results:?}");
    }
}

#[test]
fn mipmaps_are_added_by_gpu_copy_and_max_size_scales_during_decode() {
    let (loader, _) = setup(vec![imageio(true)]);
    let with_mips = block_on(loader.decode(
        png_3x2(),
        DecodeOptions {
            mipmaps: true,
            ..Default::default()
        },
    ))
    .unwrap();
    assert_eq!(
        with_mips.texture().format(),
        wgpu::TextureFormat::Rgba8Unorm,
        "a shared BGRA texture is copied into the renderer's format to gain mips"
    );
    assert_eq!(with_mips.mip_levels(), 2);

    let small = block_on(loader.decode(
        png_3x2(),
        DecodeOptions {
            max_size: Some([1, 1]),
            ..Default::default()
        },
    ))
    .unwrap();
    assert_eq!(small.size(), [1, 1]);
}

#[test]
fn a_one_frame_animation_keeps_its_timing_and_loop_count() {
    let mut bytes = Vec::new();
    {
        let mut encoder = image::codecs::gif::GifEncoder::new(&mut bytes);
        encoder
            .set_repeat(image::codecs::gif::Repeat::Finite(2))
            .unwrap();
        encoder
            .encode_frame(image::Frame::from_parts(
                image::RgbaImage::from_pixel(1, 1, image::Rgba([255; 4])),
                0,
                0,
                image::Delay::from_numer_denom_ms(80, 1),
            ))
            .unwrap();
    }
    let (loader, _) = setup(vec![imageio(true), Box::new(SoftwareDecoder)]);
    let codec = block_on(loader.open(bytes.into(), DecodeOptions::default())).unwrap();
    assert_eq!(codec.info().repetition, Repetition::Times(2));
    let frame = block_on(codec.next_frame()).unwrap();
    assert_eq!(frame.duration, Duration::from_millis(80));
    // A single frame that carries animation metadata is still the platform decoder's to read,
    // and the shared path says it was the one that read it.
    assert_eq!(
        frame.image.texture().format(),
        wgpu::TextureFormat::Bgra8Unorm
    );
}

#[test]
fn exif_orientation_is_applied_on_the_shared_path() {
    let mut bytes = Vec::new();
    let mut encoder = image::codecs::png::PngEncoder::new(&mut bytes);
    // EXIF orientation 6: rotate 90° clockwise.
    encoder
        .set_exif_metadata(vec![
            b'I', b'I', 42, 0, 8, 0, 0, 0, 1, 0, 0x12, 1, 3, 0, 1, 0, 0, 0, 6, 0, 0, 0, 0, 0, 0, 0,
        ])
        .unwrap();
    encoder
        .write_image(
            &[255, 0, 0, 255, 0, 255, 0, 255],
            2,
            1,
            image::ExtendedColorType::Rgba8,
        )
        .unwrap();
    let (loader, mut context) = setup(vec![imageio(true)]);
    let codec = block_on(loader.open(bytes.into(), DecodeOptions::default())).unwrap();
    assert_eq!(codec.info().size, [1, 2]);
    let frame = block_on(codec.next_frame()).unwrap();
    assert_eq!(frame.image.size(), [1, 2]);
    assert_eq!(
        frame.image.texture().format(),
        wgpu::TextureFormat::Bgra8Unorm
    );
    assert_eq!(
        render(&mut context, &frame.image),
        [255, 0, 0, 255, 0, 255, 0, 255]
    );
}

fn animated_webp(frame_count: u32) -> Arc<[u8]> {
    fn chunk(bytes: &mut Vec<u8>, kind: &[u8; 4], data: &[u8]) {
        bytes.extend_from_slice(kind);
        bytes.extend_from_slice(&(data.len() as u32).to_le_bytes());
        bytes.extend_from_slice(data);
        if !data.len().is_multiple_of(2) {
            bytes.push(0);
        }
    }
    // WebP RIFF ANIM/ANMF layout, also read by image-webp's container parser.
    let mut body = b"WEBP".to_vec();
    chunk(&mut body, b"VP8X", &[2, 0, 0, 0, 0, 0, 0, 0, 0, 0]);
    chunk(&mut body, b"ANIM", &[0, 0, 0, 0, 3, 0]);
    for index in 0..frame_count {
        let mut still = Vec::new();
        image::codecs::webp::WebPEncoder::new_lossless(&mut still)
            .write_image(
                &[255, index as u8 * 100, 0, 255],
                1,
                1,
                image::ExtendedColorType::Rgba8,
            )
            .unwrap();
        assert_eq!(&still[12..16], b"VP8L");
        let mut frame = vec![0; 16];
        frame[12] = 70;
        frame[15] = 2;
        frame.extend_from_slice(&still[12..]);
        chunk(&mut body, b"ANMF", &frame);
    }
    let mut bytes = b"RIFF".to_vec();
    bytes.extend_from_slice(&(body.len() as u32).to_le_bytes());
    bytes.extend_from_slice(&body);
    bytes.into()
}

#[test]
fn webp_animations_keep_loop_counts_with_and_without_a_native_decoder() {
    for count in [1, 2] {
        for native in [false, true] {
            let mut decoders: Vec<Box<dyn Decoder>> = Vec::new();
            if native {
                decoders.push(imageio(true));
            }
            decoders.push(Box::new(SoftwareDecoder));
            let (loader, _) = setup(decoders);
            let codec =
                block_on(loader.open(animated_webp(count), DecodeOptions::default())).unwrap();
            assert_eq!(codec.info().frame_count, count);
            assert_eq!(codec.info().repetition, Repetition::Times(2));
            // The shared texture says the platform decoder read it, plain RGBA that software did.
            let expected_format = if native {
                wgpu::TextureFormat::Bgra8Unorm
            } else {
                wgpu::TextureFormat::Rgba8Unorm
            };
            for _ in 0..count + 1 {
                let frame = block_on(codec.next_frame()).unwrap();
                assert_eq!(frame.duration, Duration::from_millis(70));
                assert_eq!(frame.image.texture().format(), expected_format);
            }
        }
    }
}

#[test]
fn unrecognised_bytes_are_declined_not_reported_as_damaged() {
    let (loader, _) = setup(vec![imageio(true)]);
    match block_on(loader.decode(
        Arc::from(*b"definitely not an image"),
        DecodeOptions::default(),
    )) {
        Err(DecodeError::Unsupported(declined)) => assert_eq!(declined[0].decoder, "imageio"),
        other => panic!("{other:?}"),
    }
}

/// A two-by-one animation whose frames are partial and whose disposal covers all three rules:
/// keep what is there, restore what was there before, and clear back to the background. The
/// software decoder composites these itself and is tested on the same bytes, so agreement here
/// is agreement about composition, not just about decoding.
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
fn the_platform_composites_an_animation_and_reports_its_timing() {
    let (loader, mut context) = setup(vec![imageio(true)]);
    let codec = block_on(loader.open(animated_gif(), DecodeOptions::default())).unwrap();
    assert_eq!(codec.info().frame_count, 4);
    assert_eq!(codec.info().repetition, Repetition::Times(2));
    let frames: Vec<_> = (0..5)
        .map(|_| block_on(codec.next_frame()).unwrap())
        .collect();
    drop(codec);
    // The fifth request wraps to the first frame, which is what a looping animation asks for.
    let expected = [
        [255, 0, 0, 255, 255, 0, 0, 255],
        [0, 255, 0, 255, 255, 0, 0, 255],
        [255, 0, 0, 255, 0, 0, 255, 255],
        [0, 255, 0, 255, 0, 0, 0, 0],
        [255, 0, 0, 255, 255, 0, 0, 255],
    ];
    for (index, (frame, expected)) in frames.iter().zip(expected).enumerate() {
        assert_eq!(frame.duration, Duration::from_millis(70), "frame {index}");
        assert_eq!(
            render(&mut context, &frame.image),
            expected,
            "frame {index}"
        );
    }
}

#[test]
fn the_platform_and_software_decoders_agree_on_every_frame() {
    let mut rendered = Vec::new();
    for decoders in [
        vec![imageio(true)],
        vec![Box::new(SoftwareDecoder) as Box<dyn Decoder>],
    ] {
        let (loader, mut context) = setup(decoders);
        let codec = block_on(loader.open(animated_gif(), DecodeOptions::default())).unwrap();
        let frames: Vec<_> = (0..4)
            .map(|_| render(&mut context, &block_on(codec.next_frame()).unwrap().image))
            .collect();
        drop(codec);
        rendered.push(frames);
    }
    assert_eq!(rendered[0], rendered[1]);
}
