//! The call log: every call a builder is given, as given, whatever the builder then
//! records of it.

use std::sync::Arc;

use valo_dl::{
    Backdrop, Call, ClipOp, DisplayList, DisplayListBuilder, GlyphPos, Image, ImageFilter,
    MaskKind, Paint, RRectRadii, RunFont, RunText, Sampling,
};
use valo_geometry::{Color, FillRule, Matrix, PathBuilder, Point, Rect};
use valo_text::{Font, FontSynthesis};

fn rect(l: f32, t: f32, r: f32, b: f32) -> Rect {
    Rect::from_ltrb(l, t, r, b)
}

fn red() -> Paint {
    Paint::from_color(Color::rgba(1.0, 0.0, 0.0, 1.0))
}

/// An image on a device with no GPU behind it: the log compares images by identity.
fn image() -> Image {
    let (device, _queue) = wgpu::Device::noop(&wgpu::DeviceDescriptor::default());
    let texture = device.create_texture(&wgpu::TextureDescriptor {
        label: None,
        size: wgpu::Extent3d {
            width: 4,
            height: 2,
            depth_or_array_layers: 1,
        },
        mip_level_count: 1,
        sample_count: 1,
        dimension: wgpu::TextureDimension::D2,
        format: wgpu::TextureFormat::Rgba8Unorm,
        usage: wgpu::TextureUsages::TEXTURE_BINDING,
        view_formats: &[],
    });
    Image::from_texture(texture, [4, 2], 1)
}

fn run_font() -> RunFont {
    let path = concat!(
        env!("CARGO_MANIFEST_DIR"),
        "/../../assets/fonts/fira_sans.ttf"
    );
    let face = Font::from_bytes(std::fs::read(path).unwrap()).unwrap();
    RunFont {
        face: Arc::new(face),
        size: 12.0,
        synthesis: FontSynthesis::NONE,
    }
}

fn nested_list() -> Arc<DisplayList> {
    let mut builder = DisplayListBuilder::new();
    builder.draw_rect(rect(0.0, 0.0, 1.0, 1.0), &red());
    Arc::new(builder.build())
}

/// Every public recording method of the builder, called once, and the call each logs.
fn every_call(builder: &mut DisplayListBuilder) -> Vec<Call> {
    let image = image();
    let font = run_font();
    let list = nested_list();
    let path = {
        let mut p = PathBuilder::new();
        p.circle(Point::new(5.0, 5.0), 3.0);
        p.build()
    };
    let glyphs = Arc::new(vec![GlyphPos {
        id: 3,
        x: 1.0,
        y: 10.0,
    }]);
    let text = RunText {
        text: Arc::from("a"),
        clusters: Arc::from([0u32]),
    };
    let backdrop = Backdrop::new(ImageFilter::blur(2.0, 2.0));
    let matrix = Matrix::scale(2.0, 3.0);
    let corners = [1.0, 2.0, 3.0, 4.0];
    let elliptical = [[1.0, 2.0], [2.0, 3.0], [3.0, 4.0], [4.0, 5.0]];
    let sampling = Sampling::default();

    builder.save();
    builder.save_layer(Some(rect(0.0, 0.0, 9.0, 9.0)), &red());
    builder.save_layer_mask(None, MaskKind::Alpha);
    builder.save_layer_backdrop(None, &red(), backdrop.clone());
    builder.restore();
    builder.translate(1.0, 2.0);
    builder.scale(3.0, 4.0);
    builder.rotate(0.5);
    builder.concat(&matrix);
    builder.clip_rect(rect(0.0, 0.0, 50.0, 50.0), ClipOp::Intersect, false);
    builder.clip_rrect(rect(0.0, 0.0, 40.0, 40.0), 2.0, ClipOp::Intersect, true);
    builder.clip_rrect_radii(
        rect(0.0, 0.0, 30.0, 30.0),
        corners,
        ClipOp::Difference,
        true,
    );
    builder.clip_rrect_radii_elliptical(
        rect(0.0, 0.0, 20.0, 20.0),
        elliptical,
        ClipOp::Intersect,
        true,
    );
    builder.clip_path(&path, FillRule::EvenOdd, ClipOp::Intersect, true);
    builder.draw_paint(&red());
    builder.draw_shadow(&path, FillRule::NonZero, Color::BLACK, 4.0, false, 2.0);
    builder.draw_rect(rect(1.0, 1.0, 2.0, 2.0), &red());
    builder.draw_rrect(rect(1.0, 1.0, 5.0, 5.0), 1.0, &red());
    builder.draw_rrect_radii(rect(1.0, 1.0, 6.0, 6.0), corners, &red());
    builder.draw_rrect_radii_elliptical(rect(1.0, 1.0, 7.0, 7.0), elliptical, &red());
    builder.draw_circle(Point::new(3.0, 3.0), 2.0, &red());
    builder.draw_path(&path, FillRule::NonZero, &red());
    builder.draw_image(&image, rect(0.0, 0.0, 4.0, 2.0), &red());
    builder.draw_image_rect(
        &image,
        rect(0.0, 0.0, 2.0, 2.0),
        rect(0.0, 0.0, 8.0, 8.0),
        sampling,
        &red(),
    );
    builder.draw_glyph_run(
        font.clone(),
        &red(),
        Arc::clone(&glyphs),
        Some(text.clone()),
        rect(0.0, 0.0, 8.0, 12.0),
    );
    builder.draw_display_list(&list);
    builder.draw_display_list_cached(&list);

    vec![
        Call::Save,
        Call::SaveLayer {
            bounds_hint: Some(rect(0.0, 0.0, 9.0, 9.0)),
            paint: red(),
        },
        Call::SaveLayerMask {
            bounds_hint: None,
            kind: MaskKind::Alpha,
        },
        Call::SaveLayerBackdrop {
            bounds_hint: None,
            paint: red(),
            backdrop,
        },
        Call::Restore,
        Call::Translate { tx: 1.0, ty: 2.0 },
        Call::Scale { sx: 3.0, sy: 4.0 },
        Call::Rotate { radians: 0.5 },
        Call::Concat { matrix },
        Call::ClipRect {
            rect: rect(0.0, 0.0, 50.0, 50.0),
            op: ClipOp::Intersect,
            anti_alias: false,
        },
        Call::ClipRRect {
            rect: rect(0.0, 0.0, 40.0, 40.0),
            radii: RRectRadii::Uniform(2.0),
            op: ClipOp::Intersect,
            anti_alias: true,
        },
        Call::ClipRRect {
            rect: rect(0.0, 0.0, 30.0, 30.0),
            radii: RRectRadii::Circular(corners),
            op: ClipOp::Difference,
            anti_alias: true,
        },
        Call::ClipRRect {
            rect: rect(0.0, 0.0, 20.0, 20.0),
            radii: RRectRadii::Elliptical(elliptical),
            op: ClipOp::Intersect,
            anti_alias: true,
        },
        Call::ClipPath {
            path: Arc::clone(&path),
            fill_rule: FillRule::EvenOdd,
            op: ClipOp::Intersect,
            anti_alias: true,
        },
        Call::DrawPaint { paint: red() },
        Call::DrawShadow {
            path: Arc::clone(&path),
            fill_rule: FillRule::NonZero,
            color: Color::BLACK,
            elevation: 4.0,
            transparent_occluder: false,
            device_pixel_ratio: 2.0,
        },
        Call::DrawRect {
            rect: rect(1.0, 1.0, 2.0, 2.0),
            paint: red(),
        },
        Call::DrawRRect {
            rect: rect(1.0, 1.0, 5.0, 5.0),
            radii: RRectRadii::Uniform(1.0),
            paint: red(),
        },
        Call::DrawRRect {
            rect: rect(1.0, 1.0, 6.0, 6.0),
            radii: RRectRadii::Circular(corners),
            paint: red(),
        },
        Call::DrawRRect {
            rect: rect(1.0, 1.0, 7.0, 7.0),
            radii: RRectRadii::Elliptical(elliptical),
            paint: red(),
        },
        Call::DrawCircle {
            center: Point::new(3.0, 3.0),
            radius: 2.0,
            paint: red(),
        },
        Call::DrawPath {
            path: Arc::clone(&path),
            fill_rule: FillRule::NonZero,
            paint: red(),
        },
        Call::DrawImage {
            image: image.clone(),
            dst: rect(0.0, 0.0, 4.0, 2.0),
            paint: red(),
        },
        Call::DrawImageRect {
            image,
            src: rect(0.0, 0.0, 2.0, 2.0),
            dst: rect(0.0, 0.0, 8.0, 8.0),
            sampling,
            paint: red(),
        },
        Call::DrawGlyphRun {
            font,
            paint: red(),
            glyphs,
            text: Some(text),
            local_bounds: rect(0.0, 0.0, 8.0, 12.0),
        },
        Call::DrawDisplayList {
            list: Arc::clone(&list),
            cached: false,
        },
        Call::DrawDisplayList { list, cached: true },
    ]
}

#[test]
fn every_call_is_logged_once_as_given() {
    let mut builder = DisplayListBuilder::with_call_log();
    let expected = every_call(&mut builder);
    assert_eq!(builder.call_log(), expected.as_slice());
    let (_list, calls) = builder.build_with_call_log();
    assert_eq!(calls, expected);
}

#[test]
fn a_builder_made_with_new_logs_nothing() {
    let mut builder = DisplayListBuilder::new();
    every_call(&mut builder);
    assert!(builder.call_log().is_empty());
    let (list, calls) = builder.build_with_call_log();
    assert!(calls.is_empty());
    assert!(!list.ops().is_empty(), "the calls were still recorded");
}

/// A transparent draw, an empty rectangle and a restore with no save to close leave nothing
/// in the list, and are in the log as made.
#[test]
fn a_call_the_builder_drops_is_still_logged() {
    let mut builder = DisplayListBuilder::with_call_log();
    let transparent = Paint::from_color(Color::TRANSPARENT);
    builder.draw_rect(rect(0.0, 0.0, 10.0, 10.0), &transparent);
    builder.draw_rect(rect(5.0, 5.0, 5.0, 9.0), &red());
    builder.restore();
    let (list, calls) = builder.build_with_call_log();

    assert!(list.ops().is_empty(), "the builder dropped every call");
    assert_eq!(
        calls,
        vec![
            Call::DrawRect {
                rect: rect(0.0, 0.0, 10.0, 10.0),
                paint: transparent,
            },
            Call::DrawRect {
                rect: rect(5.0, 5.0, 5.0, 9.0),
                paint: red(),
            },
            Call::Restore,
        ]
    );
}

/// Replaying an op logs the call replay makes for it, and nothing of its own.
#[test]
fn replay_logs_the_calls_it_makes() {
    let mut recorded = DisplayListBuilder::new();
    recorded.draw_rect(rect(0.0, 0.0, 3.0, 3.0), &red());
    let list = recorded.build();

    let mut builder = DisplayListBuilder::with_call_log();
    for op in list.ops() {
        builder.replay(op);
    }
    assert_eq!(
        builder.call_log(),
        &[Call::DrawRect {
            rect: rect(0.0, 0.0, 3.0, 3.0),
            paint: red(),
        }]
    );
}
