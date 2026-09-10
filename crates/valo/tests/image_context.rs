use valo::{
    AlphaType, Color, Context, DisplayListBuilder, ImageContext, ImageDesc, ImageError, Paint,
    PixelBuffer, PixelFormat, PixelLayout, Rect,
};

fn setup() -> (ImageContext, Context) {
    let (device, queue) = valo_harness::headless_device().unwrap();
    let context = Context::new(device, queue);
    (context.image_context(), context)
}

fn draw(context: &mut Context, image: &valo::Image) -> Vec<u8> {
    let size = image.size();
    let mut scene = DisplayListBuilder::new();
    scene.draw_image(
        image,
        Rect::new(0.0, 0.0, size[0] as f32, size[1] as f32),
        &Paint::from_color(Color::WHITE),
    );
    context.render_to_rgba(&scene.build(), size, Some(Color::TRANSPARENT))
}

#[test]
fn padded_bgra_straight_rows_upload_like_packed_premultiplied_rgba() {
    let (images, mut context) = setup();
    let pixels = PixelBuffer::new(
        PixelLayout {
            size: [1, 2],
            row_bytes: 8,
            format: PixelFormat::Bgra8,
            alpha: AlphaType::Straight,
        },
        vec![0, 0, 255, 255, 9, 9, 9, 9, 0, 255, 0, 128, 9, 9, 9, 9],
    )
    .unwrap();
    let image = images.upload_pixels(pixels, false).unwrap();
    let rgba = context.upload_image(
        ImageDesc {
            size: [1, 2],
            premultiplied: true,
            mips: false,
        },
        &[255, 0, 0, 255, 0, 128, 0, 128],
    );
    assert_eq!(draw(&mut context, &image), [255, 0, 0, 255, 0, 255, 0, 128]);
    assert_eq!(draw(&mut context, &image), draw(&mut context, &rgba));
}

#[test]
fn pixel_buffer_rejects_a_length_that_does_not_match_its_layout() {
    let layout = PixelLayout::packed([2, 1], PixelFormat::Rgba8, AlphaType::Straight);
    assert!(matches!(
        PixelBuffer::new(layout, vec![0; 4]),
        Err(ImageError::InvalidLayout)
    ));
    assert!(PixelBuffer::new(layout, vec![0; 8]).is_ok());
}

#[test]
fn imported_texture_gains_requested_mips_and_keeps_its_pixels() {
    let (images, mut context) = setup();
    let source = context.upload_image(
        ImageDesc {
            size: [2, 2],
            premultiplied: true,
            mips: false,
        },
        &[
            255, 0, 0, 255, 0, 255, 0, 255, 0, 0, 255, 255, 255, 255, 255, 255,
        ],
    );
    let imported = images
        .import_texture(source.texture().clone(), true)
        .unwrap();
    assert_eq!(imported.mip_levels(), 2);
    assert_eq!(draw(&mut context, &imported), draw(&mut context, &source));
    let plain = images
        .import_texture(source.texture().clone(), false)
        .unwrap();
    assert_eq!(plain.mip_levels(), 1);
}

#[test]
fn import_rejects_textures_the_image_shader_cannot_sample() {
    let (images, _) = setup();
    let depth = images.device().create_texture(&wgpu::TextureDescriptor {
        label: None,
        size: wgpu::Extent3d {
            width: 1,
            height: 1,
            depth_or_array_layers: 1,
        },
        mip_level_count: 1,
        sample_count: 1,
        dimension: wgpu::TextureDimension::D2,
        format: wgpu::TextureFormat::R8Unorm,
        usage: wgpu::TextureUsages::TEXTURE_BINDING,
        view_formats: &[],
    });
    assert!(matches!(
        images.import_texture(depth, false),
        Err(ImageError::IncompatibleTexture)
    ));
}
