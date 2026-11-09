//! Plus-darker drawn on the GPU over known backdrops gives the values of
//! Core Image's `_plusDarker` kernel, which Core Graphics'
//! `kCGBlendModePlusDarker` and Core Animation's `plusD` draw too — on a
//! normalized target and on a float one, where the blend unit clamps
//! nothing, and both as a solid draw's own blend and as a layer's composite.

use valo::{BlendMode, Color, Context, DisplayListBuilder, Paint, Rect, RenderTarget};

/// `GREY` is the grey macOS draws a sidebar header's text in, in light mode.
const GREY: f32 = 0.749;

/// `CELL` is the side of the square each case is drawn in.
const CELL: u32 = 4;

/// `Case` is one source drawn with plus-darker over one backdrop, both
/// straight colours, and the premultiplied colour the kernel gives.
struct Case {
    name: &'static str,
    backdrop: Color,
    source: Color,
    expected: [f32; 4],
}

/// `cases` are the kernel's values, worked by hand:
/// `Ra = min(1, Sa + Da)`, `Rc = Ra − min(1, (Sa − Sc) + (Da − Dc))`.
fn cases() -> [Case; 7] {
    let grey = Color::rgb(GREY, GREY, GREY);
    let half_grey = Color::rgba(GREY, GREY, GREY, 0.5);
    [
        Case {
            name: "grey over white",
            backdrop: Color::WHITE,
            source: grey,
            expected: [0.749, 0.749, 0.749, 1.0],
        },
        Case {
            name: "grey over mid grey",
            backdrop: Color::rgb(0.5, 0.5, 0.5),
            source: grey,
            expected: [0.249, 0.249, 0.249, 1.0],
        },
        Case {
            name: "grey over a saturated colour",
            backdrop: Color::rgb(1.0, 0.6, 0.2),
            source: grey,
            expected: [0.749, 0.349, 0.0, 1.0],
        },
        Case {
            name: "half-transparent grey over white",
            backdrop: Color::WHITE,
            source: half_grey,
            expected: [0.8745, 0.8745, 0.8745, 1.0],
        },
        Case {
            name: "grey over transparent",
            backdrop: Color::TRANSPARENT,
            source: grey,
            expected: [0.749, 0.749, 0.749, 1.0],
        },
        Case {
            name: "grey over a half-transparent colour",
            backdrop: Color::rgba(0.2, 0.4, 0.8, 0.5),
            source: grey,
            expected: [0.349, 0.449, 0.649, 1.0],
        },
        Case {
            name: "half-transparent grey over a half-transparent colour",
            backdrop: Color::rgba(0.2, 0.4, 0.8, 0.5),
            source: half_grey,
            expected: [0.4745, 0.5745, 0.7745, 1.0],
        },
    ]
}

/// `Composite` is how the plus-darker source reaches the target.
#[derive(Clone, Copy, Debug)]
enum Composite {
    /// The rect's own paint blends plus-darker in its fragment.
    Draw,
    /// The rect draws plainly into a layer the layer's paint composites
    /// with plus-darker.
    Layer,
}

const COMPOSITES: [Composite; 2] = [Composite::Draw, Composite::Layer];

/// `scene` draws every case in a row of cells, one row per composite.
fn scene() -> valo::DisplayList {
    let mut builder = DisplayListBuilder::new();
    for (row, composite) in COMPOSITES.into_iter().enumerate() {
        for (column, case) in cases().iter().enumerate() {
            let cell = Rect::new(
                (column as u32 * CELL) as f32,
                (row as u32 * CELL) as f32,
                CELL as f32,
                CELL as f32,
            );
            builder.draw_rect(cell, &Paint::from_color(case.backdrop));
            draw_plus_darker(&mut builder, cell, case.source, composite);
        }
    }
    builder.build()
}

fn draw_plus_darker(
    builder: &mut DisplayListBuilder,
    cell: Rect,
    source: Color,
    composite: Composite,
) {
    let plus_darker = |color| Paint {
        color,
        blend_mode: BlendMode::PlusDarker,
        ..Default::default()
    };
    match composite {
        Composite::Draw => builder.draw_rect(cell, &plus_darker(source)),
        Composite::Layer => {
            builder.save_layer(Some(cell), &plus_darker(Color::BLACK));
            builder.draw_rect(cell, &Paint::from_color(source));
            builder.restore();
        }
    }
}

fn size() -> [u32; 2] {
    [cases().len() as u32 * CELL, COMPOSITES.len() as u32 * CELL]
}

/// `center` is the pixel index at the middle of a case's cell.
fn center(column: usize, row: usize) -> usize {
    let [width, _] = size();
    let (x, y) = (
        column as u32 * CELL + CELL / 2,
        row as u32 * CELL + CELL / 2,
    );
    (y * width + x) as usize
}

fn context(test: &str) -> Option<(Context, wgpu::Device, wgpu::Queue)> {
    let Some((device, queue)) = valo_harness::headless_device() else {
        eprintln!("SKIP {test}: no GPU adapter");
        return None;
    };
    Some((Context::new(device.clone(), queue.clone()), device, queue))
}

/// On RGBA8 every channel is the kernel's value within a step of rounding.
#[test]
fn plus_darker_on_a_normalized_target_follows_core_images_kernel() {
    let Some((mut context, device, queue)) =
        context("plus_darker_on_a_normalized_target_follows_core_images_kernel")
    else {
        return;
    };
    let offscreen = valo::Offscreen::new(&device, size());
    context.render(&scene(), &offscreen.target(Some(Color::TRANSPARENT)));
    let pixels = valo_harness::read_texture_rgba(&device, &queue, offscreen.texture(), size());
    for (row, composite) in COMPOSITES.into_iter().enumerate() {
        for (column, case) in cases().iter().enumerate() {
            let at = center(column, row) * 4;
            let actual = &pixels[at..at + 4];
            let expected = case.expected.map(|channel| (channel * 255.0).round() as u8);
            assert!(
                actual.iter().zip(expected).all(|(a, e)| a.abs_diff(e) <= 1),
                "{composite:?}, {}: {actual:?}, expected {expected:?}",
                case.name
            );
        }
    }
}

/// On a float target nothing clamps the blend unit's arithmetic, so the
/// kernel's own clamps must hold: a channel the grey darkens past black
/// stays at zero instead of going negative.
#[test]
fn plus_darker_on_a_float_target_follows_core_images_kernel() {
    let Some((mut context, device, queue)) =
        context("plus_darker_on_a_float_target_follows_core_images_kernel")
    else {
        return;
    };
    let texture = float_texture(&device, size());
    let view = texture.create_view(&wgpu::TextureViewDescriptor::default());
    context.render(
        &scene(),
        &RenderTarget {
            view: &view,
            texture: &texture,
            format: FLOAT_FORMAT,
            size: size(),
            clear: Some(Color::TRANSPARENT),
        },
    );
    let pixels = read_float_texture(&device, &queue, &texture, size());
    for (row, composite) in COMPOSITES.into_iter().enumerate() {
        for (column, case) in cases().iter().enumerate() {
            let actual = pixels[center(column, row)];
            assert!(
                actual
                    .iter()
                    .zip(case.expected)
                    .all(|(a, e)| (a - e).abs() <= 2e-3),
                "{composite:?}, {}: {actual:?}, expected {:?}",
                case.name,
                case.expected
            );
        }
    }
}

const FLOAT_FORMAT: wgpu::TextureFormat = wgpu::TextureFormat::Rgba16Float;

fn float_texture(device: &wgpu::Device, size: [u32; 2]) -> wgpu::Texture {
    device.create_texture(&wgpu::TextureDescriptor {
        label: Some("plus_darker.float_target"),
        size: wgpu::Extent3d {
            width: size[0],
            height: size[1],
            depth_or_array_layers: 1,
        },
        mip_level_count: 1,
        sample_count: 1,
        dimension: wgpu::TextureDimension::D2,
        format: FLOAT_FORMAT,
        usage: wgpu::TextureUsages::RENDER_ATTACHMENT | wgpu::TextureUsages::COPY_SRC,
        view_formats: &[],
    })
}

/// `read_float_texture` reads an RGBA16 float texture back, one array of
/// four channels per pixel, row by row.
fn read_float_texture(
    device: &wgpu::Device,
    queue: &wgpu::Queue,
    texture: &wgpu::Texture,
    size: [u32; 2],
) -> Vec<[f32; 4]> {
    const BYTES_PER_PIXEL: u32 = 8;
    let [width, height] = size;
    let bytes_per_row =
        (width * BYTES_PER_PIXEL).next_multiple_of(wgpu::COPY_BYTES_PER_ROW_ALIGNMENT);
    let buffer = device.create_buffer(&wgpu::BufferDescriptor {
        label: Some("plus_darker.readback"),
        size: bytes_per_row as u64 * height as u64,
        usage: wgpu::BufferUsages::COPY_DST | wgpu::BufferUsages::MAP_READ,
        mapped_at_creation: false,
    });
    let mut encoder =
        device.create_command_encoder(&wgpu::CommandEncoderDescriptor { label: None });
    encoder.copy_texture_to_buffer(
        texture.as_image_copy(),
        wgpu::TexelCopyBufferInfo {
            buffer: &buffer,
            layout: wgpu::TexelCopyBufferLayout {
                offset: 0,
                bytes_per_row: Some(bytes_per_row),
                rows_per_image: None,
            },
        },
        texture.size(),
    );
    queue.submit(std::iter::once(encoder.finish()));
    let slice = buffer.slice(..);
    slice.map_async(wgpu::MapMode::Read, |result| result.expect("map readback"));
    device
        .poll(wgpu::PollType::wait_indefinitely())
        .expect("poll");
    let data = slice.get_mapped_range().expect("readback maps");
    (0..height)
        .flat_map(|row| {
            let start = (row * bytes_per_row) as usize;
            data[start..start + (width * BYTES_PER_PIXEL) as usize]
                .chunks_exact(BYTES_PER_PIXEL as usize)
                .map(|pixel| {
                    std::array::from_fn(|channel| {
                        half_to_f32(u16::from_le_bytes([
                            pixel[channel * 2],
                            pixel[channel * 2 + 1],
                        ]))
                    })
                })
                .collect::<Vec<_>>()
        })
        .collect()
}

/// `half_to_f32` decodes one IEEE 754 binary16 value.
fn half_to_f32(bits: u16) -> f32 {
    let sign = if bits & 0x8000 == 0 { 1.0 } else { -1.0 };
    let exponent = i32::from((bits >> 10) & 0x1f);
    let mantissa = f32::from(bits & 0x3ff);
    match exponent {
        0 => sign * mantissa * 2f32.powi(-24),
        31 if mantissa == 0.0 => sign * f32::INFINITY,
        31 => f32::NAN,
        _ => sign * (1.0 + mantissa / 1024.0) * 2f32.powi(exponent - 15),
    }
}
