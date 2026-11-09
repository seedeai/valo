//! One device driving many canvases.
//!
//! This is the claim valo's landing page rests on: a traditional 2D context
//! carries its own GPU device and browsers cap those around 16, so a dozen
//! live demos is near the ceiling before anything is drawn. Attaching them all
//! to one [`Context`] shares the glyph atlas, the image cache and the
//! render-target pool.
//!
//! Every assertion here is about RESOURCES rather than pixels, deliberately.
//! A test that only checked what was drawn would pass just as happily on
//! twelve separate devices, which is the arrangement this exists to rule out.

use std::sync::Arc;

use valo::{
    Color, Context, DisplayList, DisplayListBuilder, FontCollection, ImageDesc, Paint,
    ParagraphBuilder, PersistentCanvas, Rect, TextStyle,
};

const FORMAT: wgpu::TextureFormat = wgpu::TextureFormat::Rgba8Unorm;
const CANVASES: usize = 12;
const SIZE: [u32; 2] = [256, 256];

fn fonts() -> FontCollection {
    let dir = concat!(env!("CARGO_MANIFEST_DIR"), "/../../assets/fonts");
    let mut collection = FontCollection::new();
    collection
        .register(
            "Fira Sans",
            std::fs::read(format!("{dir}/fira_sans.ttf")).unwrap(),
        )
        .unwrap();
    collection
}

/// One card's worth of work: some text, an image, and a blurred shape. Text
/// fills the glyph atlas, the image fills the image cache, the blur takes a
/// pooled filter target — one per shared subsystem.
fn card(collection: &mut FontCollection, image: &valo::Image, seed: usize) -> Arc<DisplayList> {
    let mut builder = DisplayListBuilder::new();
    builder.draw_rect(
        Rect::new(0.0, 0.0, 256.0, 256.0),
        &Paint::from_color(Color::rgb(0.1, 0.1, 0.14)),
    );
    builder.draw_image_rect(
        image,
        Rect::new(0.0, 0.0, image.width(), image.height()),
        Rect::new(16.0, 16.0, 64.0, 64.0),
        valo::Sampling::default(),
        &Paint::from_color(Color::WHITE),
    );
    builder.draw_rect(
        Rect::new(120.0, 40.0, 90.0, 90.0),
        &Paint {
            color: Color::rgb(0.9, 0.4, 0.2),
            mask_blur: Some(valo::MaskBlur::new(6.0)),
            ..Paint::default()
        },
    );

    // The SAME text on every card: the atlas is keyed by glyph, so twelve
    // cards drawing it must not raster it twelve times.
    let mut paragraph = ParagraphBuilder::new(collection);
    let mut style = TextStyle::new("Fira Sans", 24.0, Color::WHITE);
    style.families = vec!["Fira Sans".to_owned()];
    paragraph.add_text("Shared device", &style);
    let mut paragraph = paragraph.build();
    paragraph.layout(240.0);
    use valo::DrawParagraphExt;
    builder.draw_paragraph(&paragraph, (16.0, 150.0 + (seed % 3) as f32));

    Arc::new(builder.build())
}

fn swatch(context: &mut Context) -> valo::Image {
    context.upload_image(
        ImageDesc {
            size: [32, 32],
            premultiplied: true,
            mips: false,
        },
        &vec![200u8; 32 * 32 * 4],
    )
}

/// Render `count` cards on ONE context, and report what that device holds.
fn shared(device: &wgpu::Device, queue: &wgpu::Queue, count: usize) -> (u64, u64, u64, u32) {
    let mut context = Context::new(device.clone(), queue.clone());
    let mut collection = fonts();
    let image = swatch(&mut context);
    let mut canvases: Vec<PersistentCanvas> = (0..count)
        .map(|_| PersistentCanvas::new(&mut context, SIZE, FORMAT))
        .collect();
    for (seed, canvas) in canvases.iter_mut().enumerate() {
        let list = card(&mut collection, &image, seed);
        canvas.draw(&mut context, &list, Some(Color::TRANSPARENT));
    }
    let report = context.memory_report();
    (
        report.total_bytes(),
        report.atlas.iter().map(|family| family.bytes).sum(),
        report.images.bytes,
        report.targets.count,
    )
}

/// The same work with a context — a device — PER canvas, which is what a
/// per-canvas 2D context costs.
fn per_canvas(device: &wgpu::Device, queue: &wgpu::Queue, count: usize) -> (u64, u64, u64, u32) {
    let mut totals = (0u64, 0u64, 0u64, 0u32);
    for seed in 0..count {
        let mut context = Context::new(device.clone(), queue.clone());
        let mut collection = fonts();
        let image = swatch(&mut context);
        let mut canvas = PersistentCanvas::new(&mut context, SIZE, FORMAT);
        let list = card(&mut collection, &image, seed);
        canvas.draw(&mut context, &list, Some(Color::TRANSPARENT));
        let report = context.memory_report();
        totals.0 += report.total_bytes();
        totals.1 += report.atlas.iter().map(|family| family.bytes).sum::<u64>();
        totals.2 += report.images.bytes;
        totals.3 += report.targets.count;
    }
    totals
}

/// The headline: twelve canvases on one device hold ONE atlas, ONE image
/// cache and ONE target pool, where twelve devices would hold twelve of each.
#[test]
fn one_device_holds_one_copy_of_what_twelve_would_duplicate() {
    let Some((device, queue)) = valo_harness::headless_device() else {
        eprintln!("SKIP one_device_holds_one_copy_of_what_twelve_would_duplicate");
        return;
    };

    let (one_total, one_atlas, one_images, _) = shared(&device, &queue, 1);
    let (many_total, many_atlas, many_images, many_targets) = shared(&device, &queue, CANVASES);
    let (split_total, split_atlas, split_images, split_targets) =
        per_canvas(&device, &queue, CANVASES);

    // The per-canvas backing pair is NOT in valo's own accounting (it wraps
    // host textures), and it costs the same either way — so it is added to
    // both sides rather than quietly left out of the flattering one.
    let backing_pair = 2 * u64::from(SIZE[0]) * u64::from(SIZE[1]) * 4;
    let per_canvas_cost = backing_pair * CANVASES as u64;
    let mib = |bytes: u64| bytes as f64 / (1024.0 * 1024.0);
    println!(
        "\n{CANVASES} canvases, {}x{} each\n\
         \x20 shared device : {:.2} MiB device-level + {:.2} MiB backings = {:.2} MiB\n\
         \x20 device each   : {:.2} MiB device-level + {:.2} MiB backings = {:.2} MiB\n\
         \x20 atlas         : {:.2} MiB shared vs {:.2} MiB split\n\
         \x20 pooled targets: {many_targets} shared vs {split_targets} split\n\
         \x20 one canvas    : {:.2} MiB device-level\n",
        SIZE[0],
        SIZE[1],
        mib(many_total),
        mib(per_canvas_cost),
        mib(many_total + per_canvas_cost),
        mib(split_total),
        mib(per_canvas_cost),
        mib(split_total + per_canvas_cost),
        mib(many_atlas),
        mib(split_atlas),
        mib(one_total),
    );

    // The atlas is the claim that matters: it is per-device and keyed by
    // glyph, so the same text on twelve canvases rasters once.
    assert_eq!(
        many_atlas, one_atlas,
        "twelve canvases must share one glyph atlas"
    );
    assert_eq!(
        split_atlas,
        one_atlas * CANVASES as u64,
        "twelve devices each pay for their own atlas — the baseline being beaten"
    );

    // Same for uploaded images.
    assert_eq!(many_images, one_images, "one image cache, not twelve");
    assert_eq!(split_images, one_images * CANVASES as u64);

    // Pooled targets are reused across canvases rather than accumulating.
    assert!(
        many_targets <= split_targets,
        "a shared pool must not hold more targets than separate ones: \
         {many_targets} vs {split_targets}"
    );

    assert!(
        many_total < split_total,
        "sharing must actually save: {many_total} B shared vs {split_total} B split"
    );
}

/// Per-canvas state stays per-canvas: each keeps its own pixels, and drawing
/// into one leaves the others alone.
#[test]
fn canvases_on_one_device_keep_their_own_pixels() {
    let Some((device, queue)) = valo_harness::headless_device() else {
        eprintln!("SKIP canvases_on_one_device_keep_their_own_pixels");
        return;
    };
    let mut context = Context::new(device.clone(), queue.clone());
    let colors = [
        Color::rgb(1.0, 0.0, 0.0),
        Color::rgb(0.0, 1.0, 0.0),
        Color::rgb(0.0, 0.0, 1.0),
    ];
    let mut canvases: Vec<PersistentCanvas> = colors
        .iter()
        .map(|_| PersistentCanvas::new(&mut context, [32, 32], FORMAT))
        .collect();

    for (canvas, color) in canvases.iter_mut().zip(colors) {
        let mut builder = DisplayListBuilder::new();
        builder.draw_rect(Rect::new(0.0, 0.0, 32.0, 32.0), &Paint::from_color(color));
        canvas.draw(&mut context, &Arc::new(builder.build()), None);
    }

    // Draw again into only the first: the others must not move.
    let mut builder = DisplayListBuilder::new();
    builder.draw_rect(
        Rect::new(0.0, 0.0, 32.0, 32.0),
        &Paint::from_color(Color::rgb(1.0, 1.0, 1.0)),
    );
    canvases[0].draw(&mut context, &Arc::new(builder.build()), None);

    let expected = [[255, 255, 255], [0, 255, 0], [0, 0, 255]];
    for (canvas, want) in canvases.iter().zip(expected) {
        let pixels =
            valo_harness::read_texture_rgba(&device, &queue, canvas.front().texture(), [32, 32]);
        assert_eq!(&pixels[..3], &want, "each canvas keeps its own pixels");
    }
}

/// Dropping one canvas must not disturb the device or its siblings — a card
/// scrolled out of the DOM should free its backing and nothing else.
#[test]
fn dropping_one_canvas_leaves_the_others_drawing() {
    let Some((device, queue)) = valo_harness::headless_device() else {
        eprintln!("SKIP dropping_one_canvas_leaves_the_others_drawing");
        return;
    };
    let mut context = Context::new(device.clone(), queue.clone());
    let mut keep = PersistentCanvas::new(&mut context, [32, 32], FORMAT);
    let discard = PersistentCanvas::new(&mut context, [32, 32], FORMAT);

    let mut builder = DisplayListBuilder::new();
    builder.draw_rect(
        Rect::new(0.0, 0.0, 32.0, 32.0),
        &Paint::from_color(Color::rgb(0.0, 1.0, 0.0)),
    );
    let green = Arc::new(builder.build());
    keep.draw(&mut context, &green, None);
    drop(discard);

    // The survivor still renders, and its earlier pixels are still there.
    let empty = Arc::new(DisplayListBuilder::new().build());
    keep.draw(&mut context, &empty, None);
    let pixels = valo_harness::read_texture_rgba(&device, &queue, keep.front().texture(), [32, 32]);
    assert_eq!(&pixels[..3], &[0, 255, 0]);
}
