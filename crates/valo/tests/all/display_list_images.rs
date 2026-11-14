//! A display list drawn into an image (`Context::display_list_to_image`, Impeller's
//! `DisplayListToTexture`) and sampled by a runtime shader, as Flutter's `Picture.toImageSync`
//! feeds a fragment shader's sampler. Skips (with a note) when the machine has no adapter;
//! `VALO_BLESS=1 cargo test` regenerates the golden.

use valo::{
    Color, Context, DisplayListBuilder, EffectImage, Filter, Offscreen, Paint, Rect, RuntimeEffect,
    RuntimeShader, Sampling, Shader, TileMode,
};

use crate::goldens_dir;

/// The picture repeated across the shape, each copy turned further about its centre.
const TURNED_TILES: &str = "
@group(1) @binding(0) var picture: texture_2d<f32>;
@group(1) @binding(1) var picture_sampler: sampler;

fn shade(position: vec2<f32>) -> vec4<f32> {
    let tile = floor(position / 64.0);
    let angle = (tile.x + 3.0 * tile.y) * 0.3;
    let local = position - (tile + 0.5) * 64.0;
    let turned = vec2<f32>(
        cos(angle) * local.x + sin(angle) * local.y,
        -sin(angle) * local.x + cos(angle) * local.y,
    );
    return textureSampleLevel(picture, picture_sampler, turned / 64.0 + 0.5, 0.0);
}
";

#[test]
fn a_picture_drawn_to_an_image_is_sampled_by_a_shader() {
    let Some((device, queue)) = valo_harness::headless_device() else {
        eprintln!("SKIP a_picture_drawn_to_an_image_is_sampled_by_a_shader: no GPU adapter");
        return;
    };
    let mut ctx = Context::new(device.clone(), queue.clone());
    let mut picture = DisplayListBuilder::new();
    picture.draw_rect(
        Rect::new(8.0, 8.0, 48.0, 20.0),
        &Paint::from_color(Color::rgb(0.95, 0.75, 0.2)),
    );
    picture.draw_circle(
        (32.0, 42.0),
        14.0,
        &Paint::from_color(Color::rgba(0.3, 0.5, 1.0, 0.8)),
    );
    let image = ctx.display_list_to_image(&picture.build(), [64, 64]);
    assert_eq!(image.size(), [64, 64]);

    let effect = RuntimeEffect::shader(TURNED_TILES).expect("valid effect");
    let sampling = Sampling {
        filter: Filter::Linear,
        tile_x: TileMode::Decal,
        tile_y: TileMode::Decal,
        ..Sampling::default()
    };
    let images = vec![EffectImage {
        image: Some(image),
        sampling,
    }];
    let shader = RuntimeShader::new(effect, Vec::new(), images).expect("the image fits");
    let size = [256u32, 128u32];
    let mut b = DisplayListBuilder::new();
    b.draw_rect(
        Rect::new(0.0, 0.0, size[0] as f32, size[1] as f32),
        &Paint::from_shader(Shader::Runtime(shader)),
    );

    let offscreen = Offscreen::new(&device, size);
    ctx.render(
        &b.build(),
        &offscreen.target(Some(Color::rgb(0.1, 0.1, 0.12))),
    );
    let rgba = valo_harness::read_texture_rgba(&device, &queue, offscreen.texture(), size);
    let pixel = |x: u32, y: u32| {
        let at = ((y * size[0] + x) * 4) as usize;
        [rgba[at], rgba[at + 1], rgba[at + 2], rgba[at + 3]]
    };
    // The first tile is not turned: the bar's middle, and a corner the picture left clear.
    assert_eq!(pixel(32, 18), [242, 191, 51, 255]);
    assert_eq!(pixel(2, 2), [26, 26, 31, 255]);
    valo_harness::assert_golden(goldens_dir(), "display_list_image_in_a_shader", size, &rgba);
}
