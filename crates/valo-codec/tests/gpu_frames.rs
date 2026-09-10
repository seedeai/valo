//! A decoder that hands back a texture instead of pixels: the zero-copy path.
mod support;

use pollster::block_on;
use support::*;
use valo::{Color, Context, DisplayListBuilder, Paint, Rect};
use valo_codec::{
    DecodeError, DecodeOptions, DecodedFrame, Decoder, FramePixels, FrameReader, ImageInfo,
    ImageLoader, OpenError, OpenRequest, Repetition,
};

/// Writes a 2×1 red/green texture on the request's device and returns it as the frame.
struct TextureDecoder;

impl Decoder for TextureDecoder {
    fn name(&self) -> &'static str {
        "texture"
    }

    fn open(&self, request: &OpenRequest) -> Result<Box<dyn FrameReader>, OpenError> {
        Ok(Box::new(TextureReader {
            device: request.device.clone(),
        }))
    }
}

struct TextureReader {
    device: wgpu::Device,
}

impl FrameReader for TextureReader {
    fn info(&self) -> ImageInfo {
        ImageInfo {
            size: [2, 1],
            frame_count: 1,
            repetition: Repetition::Once,
        }
    }

    fn next_frame(&mut self) -> Result<DecodedFrame, DecodeError> {
        let texture = self.device.create_texture(&wgpu::TextureDescriptor {
            label: None,
            size: wgpu::Extent3d {
                width: 2,
                height: 1,
                depth_or_array_layers: 1,
            },
            mip_level_count: 1,
            sample_count: 1,
            dimension: wgpu::TextureDimension::D2,
            format: wgpu::TextureFormat::Bgra8Unorm,
            usage: wgpu::TextureUsages::TEXTURE_BINDING | wgpu::TextureUsages::COPY_DST,
            view_formats: &[],
        });
        Ok(DecodedFrame::still(FramePixels::Gpu(texture)))
    }
}

fn fill_bgra(queue: &wgpu::Queue, texture: &wgpu::Texture, bgra: &[u8]) {
    queue.write_texture(
        texture.as_image_copy(),
        bgra,
        wgpu::TexelCopyBufferLayout {
            offset: 0,
            bytes_per_row: Some(8),
            rows_per_image: None,
        },
        texture.size(),
    );
}

fn draw(context: &mut Context, image: &valo::Image) -> Vec<u8> {
    let mut scene = DisplayListBuilder::new();
    scene.draw_image(
        image,
        Rect::new(0.0, 0.0, 2.0, 1.0),
        &Paint::from_color(Color::WHITE),
    );
    context.render_to_rgba(&scene.build(), [2, 1], Some(Color::TRANSPARENT))
}

#[test]
fn a_gpu_frame_is_drawn_from_its_own_texture_and_copied_only_for_mipmaps() {
    let (device, queue) = valo_harness::headless_device().expect("headless GPU");
    let mut context = Context::new(device.clone(), queue.clone());
    let loader = ImageLoader::new(context.image_context(), vec![Box::new(TextureDecoder)]);

    let image = block_on(loader.decode(bytes(), DecodeOptions::default())).unwrap();
    assert_eq!(image.texture().format(), wgpu::TextureFormat::Bgra8Unorm);
    fill_bgra(&queue, image.texture(), &[0, 0, 255, 255, 0, 255, 0, 255]);
    assert_eq!(
        draw(&mut context, &image),
        [255, 0, 0, 255, 0, 255, 0, 255],
        "the decoder's texture is sampled directly"
    );

    let with_mips = block_on(loader.decode(
        bytes(),
        DecodeOptions {
            mipmaps: true,
            ..Default::default()
        },
    ))
    .unwrap();
    assert_eq!(
        with_mips.texture().format(),
        wgpu::TextureFormat::Rgba8Unorm
    );
    assert_eq!(with_mips.mip_levels(), 2);
}

#[test]
fn a_gpu_frame_of_the_wrong_size_is_rejected() {
    struct WrongSize;
    impl Decoder for WrongSize {
        fn name(&self) -> &'static str {
            "wrong"
        }
        fn open(&self, _: &OpenRequest) -> Result<Box<dyn FrameReader>, OpenError> {
            Ok(Box::new(WrongSizeReader))
        }
    }
    struct WrongSizeReader;
    impl FrameReader for WrongSizeReader {
        fn info(&self) -> ImageInfo {
            ImageInfo {
                size: [4, 4],
                frame_count: 1,
                repetition: Repetition::Once,
            }
        }
        fn next_frame(&mut self) -> Result<DecodedFrame, DecodeError> {
            Ok(DecodedFrame::still(FramePixels::Cpu(solid(
                [2, 2],
                [255; 4],
            ))))
        }
    }
    let loader = ImageLoader::new(images(), vec![Box::new(WrongSize)]);
    assert!(matches!(
        block_on(loader.decode(bytes(), DecodeOptions::default())),
        Err(DecodeError::InvalidData(_))
    ));
}
