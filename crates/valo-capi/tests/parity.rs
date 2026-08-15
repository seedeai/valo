//! The C ABI's proof obligations:
//! 1. A scene built THROUGH THE C FUNCTIONS renders byte-identical to the
//!    same scene built with valo's Rust API — the translation is faithful.
//! 2. The text query surface answers over the FFI like the Rust one does.
//! 3. Null handles never crash (the header's null-safety promise).
//! 4. The hand-kept header declares exactly the exported symbol set.

use std::f32::consts::FRAC_PI_8;

use valo_capi::*;

fn fira_sans_bytes() -> Vec<u8> {
    let path = concat!(
        env!("CARGO_MANIFEST_DIR"),
        "/../../assets/fonts/fira_sans.ttf"
    );
    std::fs::read(path).expect("fira_sans.ttf")
}

const CLEAR: ValoColor = ValoColor {
    red: 0.07,
    green: 0.07,
    blue: 0.09,
    alpha: 1.0,
};

fn fill(red: f32, green: f32, blue: f32) -> ValoPaint {
    ValoPaint {
        color: ValoColor {
            red,
            green,
            blue,
            alpha: 1.0,
        },
        blend_mode: 3, // srcOver
        style: 0,
        stroke_width: 0.0,
        stroke_cap: 0,
        stroke_join: 0,
        stroke_miter_limit: 0.0,
        mask_blur_style: 0,
        mask_blur_sigma: 0.0,
        color_filter: std::ptr::null(),
    }
}

fn rect(x: f32, y: f32, width: f32, height: f32) -> ValoRect {
    ValoRect {
        x,
        y,
        width,
        height,
    }
}

fn white_text_style() -> ValoTextStyle {
    ValoTextStyle {
        families_utf8: std::ptr::null(),
        families_length: 0,
        size: 18.0,
        weight: 400,
        italic: false,
        color: ValoColor {
            red: 1.0,
            green: 1.0,
            blue: 1.0,
            alpha: 1.0,
        },
        letter_spacing: 0.0,
        word_spacing: 0.0,
        line_height: 0.0,
        decoration_kind: -1,
        decoration_color: CLEAR,
        decoration_thickness: 0.0,
    }
}

fn plain_paragraph_style() -> ValoParagraphStyle {
    ValoParagraphStyle {
        align: 0,
        max_lines: 0,
        ellipsis_utf8: std::ptr::null(),
        ellipsis_length: 0,
    }
}

/// rotateX(0.35) under the Flutter perspective entry(3,2) = 0.002 —
/// composed in full 4×4 (column-major), the tilt every Flutter card demo
/// uses.
fn tilted_matrix() -> [f32; 16] {
    let (sin, cos) = 0.35_f32.sin_cos();
    #[rustfmt::skip]
    let out = [
        1.0, 0.0, 0.0,        0.0,
        0.0, cos, sin,        0.002 * sin,
        0.0, -sin, cos,       0.002 * cos,
        0.0, 0.0, 0.0,        1.0,
    ];
    out
}

fn circular(radius: f32) -> ValoCornerRadii {
    ValoCornerRadii {
        top_left_x: radius,
        top_left_y: radius,
        top_right_x: radius,
        top_right_y: radius,
        bottom_right_x: radius,
        bottom_right_y: radius,
        bottom_left_x: radius,
        bottom_left_y: radius,
    }
}

/// The same scene, twice: every drawn element through both routes.
#[test]
fn c_scene_matches_the_rust_scene_byte_for_byte() {
    let size = [360u32, 240u32];

    // ── the C route ──────────────────────────────────────────────────
    let context = valo_context_new();
    if context.is_null() {
        eprintln!("SKIP: no GPU adapter");
        return;
    }
    let builder = valo_builder_new();
    unsafe {
        valo_builder_draw_rect(
            builder,
            rect(20.0, 20.0, 100.0, 70.0),
            fill(0.3, 0.55, 0.95),
        );

        let mut elliptical = circular(18.0);
        elliptical.top_left_x = 40.0;
        elliptical.top_left_y = 12.0;
        valo_builder_draw_rounded_rect(
            builder,
            rect(140.0, 20.0, 100.0, 70.0),
            elliptical,
            fill(0.95, 0.6, 0.25),
        );

        // A stroked, rotated path with round caps under a rrect clip.
        valo_builder_save(builder);
        valo_builder_clip_rounded_rect(builder, rect(20.0, 110.0, 150.0, 100.0), circular(20.0), 0);
        valo_builder_translate(builder, 30.0, 120.0);
        valo_builder_rotate(builder, FRAC_PI_8);
        let path = valo_path_new();
        valo_path_move_to(path, 0.0, 40.0);
        valo_path_quadratic_to(path, 40.0, -20.0, 80.0, 40.0);
        valo_path_cubic_to(path, 100.0, 70.0, 20.0, 70.0, 0.0, 40.0);
        let mut stroke = fill(0.85, 0.3, 0.5);
        stroke.style = 1;
        stroke.stroke_width = 6.0;
        stroke.stroke_cap = 1;
        stroke.stroke_join = 1;
        valo_builder_draw_path(builder, path, 0, stroke);
        valo_path_dispose(path);
        valo_builder_restore(builder);

        // A translucent multiply layer over a circle + blurred shadow.
        let mut shadow = fill(0.0, 0.0, 0.0);
        shadow.color.alpha = 0.6;
        shadow.mask_blur_sigma = 5.0;
        valo_builder_draw_rounded_rect(
            builder,
            rect(206.0, 116.0, 120.0, 80.0),
            circular(14.0),
            shadow,
        );
        valo_builder_draw_rounded_rect(
            builder,
            rect(200.0, 110.0, 120.0, 80.0),
            circular(14.0),
            fill(0.92, 0.9, 0.85),
        );
        let mut layer = fill(1.0, 1.0, 1.0);
        layer.color.alpha = 0.7;
        layer.blend_mode = 24; // multiply
        valo_builder_save_layer(builder, layer);
        valo_builder_draw_circle(builder, 260.0, 150.0, 30.0, fill(0.4, 0.8, 0.5));
        valo_builder_restore(builder);

        // A perspective-tilted card (the Flutter entry(3,2) trick composed
        // with a rotateX) through the full-matrix entry.
        valo_builder_save(builder);
        valo_builder_translate(builder, 60.0, 200.0);
        valo_builder_transform_matrix(builder, tilted_matrix().as_ptr());
        valo_builder_draw_rect(builder, rect(0.0, 0.0, 120.0, 36.0), fill(0.55, 0.45, 0.9));
        valo_builder_restore(builder);

        // A colour-filtered card. The handle is released right after the
        // draw, which the header promises is enough.
        let tint = valo_color_filter_blend(
            ValoColor {
                red: 0.98,
                green: 0.55,
                blue: 0.1,
                alpha: 1.0,
            },
            5, // srcIn
        );
        let mut filtered = fill(0.2, 0.7, 0.35);
        filtered.color_filter = tint;
        valo_builder_draw_rect(builder, rect(20.0, 214.0, 90.0, 20.0), filtered);
        valo_color_filter_dispose(tint);
    }
    let list = unsafe { valo_builder_build(builder) };
    let mut c_pixels = vec![0u8; (size[0] * size[1] * 4) as usize];
    let rendered = unsafe {
        valo_context_render_to_pixels(
            context,
            list,
            CLEAR,
            size[0],
            size[1],
            c_pixels.as_mut_ptr(),
        )
    };
    assert!(rendered, "headless render reported success");
    unsafe {
        valo_display_list_dispose(list);
        valo_context_dispose(context);
    }

    // ── the Rust route (verbatim valo) ───────────────────────────────
    let Some((device, queue)) = valo_harness::headless_device() else {
        eprintln!("SKIP: no GPU adapter");
        return;
    };
    let mut context = valo::Context::new(device, queue);
    let mut b = valo::DisplayListBuilder::new();
    b.draw_rect(
        valo::Rect::new(20.0, 20.0, 100.0, 70.0),
        &valo::Paint::from_color(valo::Color::rgb(0.3, 0.55, 0.95)),
    );
    b.draw_rrect_radii_elliptical(
        valo::Rect::new(140.0, 20.0, 100.0, 70.0),
        [[40.0, 12.0], [18.0, 18.0], [18.0, 18.0], [18.0, 18.0]],
        &valo::Paint::from_color(valo::Color::rgb(0.95, 0.6, 0.25)),
    );
    b.save();
    b.clip_rrect_radii_elliptical(
        valo::Rect::new(20.0, 110.0, 150.0, 100.0),
        [[20.0; 2]; 4],
        valo::ClipOp::Intersect,
    );
    b.translate(30.0, 120.0);
    b.rotate(FRAC_PI_8);
    let mut path = valo::PathBuilder::new();
    path.move_to((0.0, 40.0))
        .quad_to((40.0, -20.0), (80.0, 40.0))
        .cubic_to((100.0, 70.0), (20.0, 70.0), (0.0, 40.0));
    b.draw_path(
        &path.build(),
        valo::FillRule::NonZero,
        &valo::Paint {
            color: valo::Color::rgb(0.85, 0.3, 0.5),
            style: valo::PaintStyle::Stroke(valo::Stroke {
                width: 6.0,
                cap: valo::Cap::Round,
                join: valo::Join::Round,
                miter_limit: 4.0,
                dash: None,
            }),
            ..valo::Paint::default()
        },
    );
    b.restore();
    b.draw_rrect_radii_elliptical(
        valo::Rect::new(206.0, 116.0, 120.0, 80.0),
        [[14.0; 2]; 4],
        &valo::Paint {
            color: valo::Color::rgba(0.0, 0.0, 0.0, 0.6),
            mask_blur: Some(valo::MaskBlur::new(5.0)),
            ..valo::Paint::default()
        },
    );
    b.draw_rrect_radii_elliptical(
        valo::Rect::new(200.0, 110.0, 120.0, 80.0),
        [[14.0; 2]; 4],
        &valo::Paint::from_color(valo::Color::rgb(0.92, 0.9, 0.85)),
    );
    b.save_layer(
        None,
        &valo::Paint {
            color: valo::Color::rgba(1.0, 1.0, 1.0, 0.7),
            blend_mode: valo::BlendMode::Multiply,
            ..valo::Paint::default()
        },
    );
    b.draw_circle(
        (260.0, 150.0),
        30.0,
        &valo::Paint::from_color(valo::Color::rgb(0.4, 0.8, 0.5)),
    );
    b.restore();
    b.save();
    b.translate(60.0, 200.0);
    b.concat(&valo::Matrix::from_flutter_array(&tilted_matrix()));
    b.draw_rect(
        valo::Rect::new(0.0, 0.0, 120.0, 36.0),
        &valo::Paint::from_color(valo::Color::rgb(0.55, 0.45, 0.9)),
    );
    b.restore();
    b.draw_rect(
        valo::Rect::new(20.0, 214.0, 90.0, 20.0),
        &valo::Paint {
            color: valo::Color::rgb(0.2, 0.7, 0.35),
            color_filter: Some(valo::ColorFilter::Blend(
                valo::Color::rgb(0.98, 0.55, 0.1),
                valo::BlendMode::SrcIn,
            )),
            ..valo::Paint::default()
        },
    );
    let rust_pixels =
        context.render_to_rgba(&b.build(), size, Some(valo::Color::rgb(0.07, 0.07, 0.09)));

    assert_eq!(
        c_pixels, rust_pixels,
        "the two routes must be pixel-identical"
    );
}

#[test]
fn text_queries_answer_over_the_ffi() {
    let fonts = valo_fonts_new();
    let bytes = fira_sans_bytes();
    let face = unsafe { valo_fonts_add(fonts, bytes.as_ptr(), bytes.len()) };
    assert!(face >= 0, "fira sans registers");
    unsafe { valo_fonts_add_fallback(fonts, face) };

    let mut name =
        vec![0u8; unsafe { valo_fonts_family_name(fonts, face, std::ptr::null_mut(), 0) }];
    unsafe { valo_fonts_family_name(fonts, face, name.as_mut_ptr(), name.len()) };
    assert_eq!(String::from_utf8(name).as_deref(), Ok("Fira Sans"));

    let family = "Fira Sans";
    let mut style = white_text_style();
    style.families_utf8 = family.as_ptr();
    style.families_length = family.len();
    let builder = unsafe { valo_paragraph_builder_new(fonts, plain_paragraph_style()) };
    let text = "hello wide world";
    unsafe { valo_paragraph_builder_add_text(builder, text.as_ptr(), text.len(), style) };
    let paragraph = unsafe { valo_paragraph_builder_build(builder, fonts as *mut _) };
    unsafe { valo_paragraph_layout(paragraph, 120.0) };

    unsafe {
        assert!(valo_paragraph_height(paragraph) > 0.0);
        assert!(valo_paragraph_line_count(paragraph) >= 2, "120px wraps");

        // Caret and hit-testing agree at the start of the text.
        let caret = valo_paragraph_caret_for_offset(paragraph, 0);
        assert!(caret.height > 0.0);
        let mut downstream = false;
        let offset =
            valo_paragraph_byte_offset_at(paragraph, caret.x + 1.0, caret.y + 1.0, &mut downstream);
        assert_eq!(offset, 0);

        // Word boundary around "wide".
        let range = valo_paragraph_word_boundary(paragraph, 7);
        assert_eq!((range.start, range.end), (6, 10));

        // Two-call rects: size, then fill.
        let total =
            valo_paragraph_rects_for_range(paragraph, 0, text.len(), std::ptr::null_mut(), 0);
        assert!(total >= 2, "one rect per wrapped line");
        let mut rects = vec![rect(0.0, 0.0, 0.0, 0.0); total];
        let written =
            valo_paragraph_rects_for_range(paragraph, 0, text.len(), rects.as_mut_ptr(), total);
        assert_eq!(written, total);
        assert!(rects.iter().all(|r| r.width > 0.0 && r.height > 0.0));

        let mut metrics = ValoLineMetrics {
            start: 0,
            end: 0,
            baseline: 0.0,
            ascent: 0.0,
            descent: 0.0,
            left: 0.0,
            width: 0.0,
        };
        assert!(valo_paragraph_line_metrics(paragraph, 0, &mut metrics));
        assert!(metrics.baseline > 0.0 && metrics.end > metrics.start);
        assert!(!valo_paragraph_line_metrics(paragraph, 999, &mut metrics));
    }

    render_matches_the_rust_route(paragraph, &bytes);
    stroked_text_matches_the_rust_route(paragraph, &bytes);

    unsafe {
        valo_paragraph_dispose(paragraph);
        valo_fonts_dispose(fonts);
    }
}

/// The paragraph must also DRAW over the FFI: a recorded glyph run carries its
/// own font instance, so the C route needs no collection at render time, and
/// the frame must match the Rust route byte-for-byte.
fn render_matches_the_rust_route(paragraph: *mut ValoParagraph, font_bytes: &[u8]) {
    let size = [140u32, 80u32];

    // ── the C route ──────────────────────────────────────────────────
    let context = valo_context_new();
    if context.is_null() {
        eprintln!("SKIP text rendering: no GPU adapter");
        return;
    }
    let mut c_pixels = vec![0u8; (size[0] * size[1] * 4) as usize];
    unsafe {
        let builder = valo_builder_new();
        valo_builder_draw_paragraph(builder, paragraph, 4.0, 4.0);
        let list = valo_builder_build(builder);
        assert!(valo_context_render_to_pixels(
            context,
            list,
            CLEAR,
            size[0],
            size[1],
            c_pixels.as_mut_ptr(),
        ));
        valo_display_list_dispose(list);
        valo_context_dispose(context);
    }

    // Ink, not just agreement: an empty frame matching an empty frame
    // must not pass.
    let clear_pixel = [
        (CLEAR.red * 255.0) as u8,
        (CLEAR.green * 255.0) as u8,
        (CLEAR.blue * 255.0) as u8,
        255,
    ];
    assert!(
        c_pixels.chunks_exact(4).any(|pixel| pixel != clear_pixel),
        "glyphs made it to the frame"
    );

    // ── the Rust route (verbatim valo) ───────────────────────────────
    let Some((device, queue)) = valo_harness::headless_device() else {
        return;
    };
    let font = valo::Font::from_bytes(font_bytes.to_vec()).expect("fira sans parses");
    let mut collection = valo::FontCollection::default();
    let id = collection.add(font);
    collection.add_fallback(id);
    let mut builder = valo::ParagraphBuilder::new(&mut collection);
    builder.style(valo::ParagraphStyle {
        align: valo::TextAlign::Left,
        preserve_trailing_whitespace: false,
        max_lines: None,
        ellipsis: None,
    });
    let mut style = valo::TextStyle::new("", 18.0, valo::Color::rgb(1.0, 1.0, 1.0));
    style.families = vec!["Fira Sans".to_owned()];
    style.weight = 400;
    builder.add_text("hello wide world", &style);
    let mut rust_paragraph = builder.build();
    rust_paragraph.layout(120.0);

    let mut context = valo::Context::new(device, queue);
    let mut b = valo::DisplayListBuilder::new();
    use valo::DrawParagraphExt;
    b.draw_paragraph(&rust_paragraph, (4.0, 4.0));
    let rust_pixels =
        context.render_to_rgba(&b.build(), size, Some(valo::Color::rgb(0.07, 0.07, 0.09)));

    assert_eq!(
        c_pixels, rust_pixels,
        "the two text routes must be pixel-identical"
    );
}

/// `ValoPaint` carries stroke fields, and `valo_builder_draw_paragraph_with`
/// is the only entry point that honours them for text — the plain
/// `valo_builder_draw_paragraph` paints each run with its own style's fill.
/// This proves the stroke survives the crossing rather than being silently
/// dropped, which is what the FFI did before this function existed.
fn stroked_text_matches_the_rust_route(paragraph: *mut ValoParagraph, font_bytes: &[u8]) {
    let size = [140u32, 80u32];
    let stroke = ValoPaint {
        style: 1, // stroke
        stroke_width: 1.5,
        stroke_join: 0, // miter
        stroke_miter_limit: 4.0,
        ..fill(1.0, 0.85, 0.2)
    };

    let context = valo_context_new();
    if context.is_null() {
        eprintln!("SKIP stroked text rendering: no GPU adapter");
        return;
    }
    let mut c_pixels = vec![0u8; (size[0] * size[1] * 4) as usize];
    let mut filled_pixels = vec![0u8; (size[0] * size[1] * 4) as usize];
    unsafe {
        let builder = valo_builder_new();
        valo_builder_draw_paragraph_with(builder, paragraph, 4.0, 4.0, stroke);
        let list = valo_builder_build(builder);
        assert!(valo_context_render_to_pixels(
            context,
            list,
            CLEAR,
            size[0],
            size[1],
            c_pixels.as_mut_ptr(),
        ));
        valo_display_list_dispose(list);

        // The same paragraph FILLED, to prove the stroke fields changed the
        // output rather than the two routes agreeing on an ignored paint.
        let builder = valo_builder_new();
        valo_builder_draw_paragraph(builder, paragraph, 4.0, 4.0);
        let list = valo_builder_build(builder);
        assert!(valo_context_render_to_pixels(
            context,
            list,
            CLEAR,
            size[0],
            size[1],
            filled_pixels.as_mut_ptr(),
        ));
        valo_display_list_dispose(list);
        valo_context_dispose(context);
    }
    assert_ne!(
        c_pixels, filled_pixels,
        "a stroked paragraph must not render identically to a filled one"
    );

    let Some((device, queue)) = valo_harness::headless_device() else {
        return;
    };
    let font = valo::Font::from_bytes(font_bytes.to_vec()).expect("fira sans parses");
    let mut collection = valo::FontCollection::default();
    let id = collection.add(font);
    collection.add_fallback(id);
    let mut builder = valo::ParagraphBuilder::new(&mut collection);
    builder.style(valo::ParagraphStyle {
        align: valo::TextAlign::Left,
        preserve_trailing_whitespace: false,
        max_lines: None,
        ellipsis: None,
    });
    let mut style = valo::TextStyle::new("", 18.0, valo::Color::rgb(1.0, 1.0, 1.0));
    style.families = vec!["Fira Sans".to_owned()];
    style.weight = 400;
    builder.add_text("hello wide world", &style);
    let mut rust_paragraph = builder.build();
    rust_paragraph.layout(120.0);

    let mut context = valo::Context::new(device, queue);
    let mut b = valo::DisplayListBuilder::new();
    use valo::DrawGlyphRunExt;
    let rust_stroke = valo::Paint {
        color: valo::Color::rgb(1.0, 0.85, 0.2),
        style: valo::PaintStyle::Stroke(valo::Stroke {
            miter_limit: 4.0,
            ..valo::Stroke::new(1.5)
        }),
        ..Default::default()
    };
    b.draw_paragraph_with(&rust_paragraph, (4.0, 4.0), &rust_stroke);
    let rust_pixels =
        context.render_to_rgba(&b.build(), size, Some(valo::Color::rgb(0.07, 0.07, 0.09)));

    assert_eq!(
        c_pixels, rust_pixels,
        "the two stroked-text routes must be pixel-identical"
    );
}

/// The demand loop over the FFI: a paragraph wanting an uninstalled-in-the-
/// collection family and an uncovered codepoint reports both, one
/// `valo_fonts_satisfy_demand` answers from the OS, and the rebuilt
/// paragraph demands nothing. Skips on machines whose installed fonts
/// cannot answer.
#[test]
fn system_fonts_answer_demands_over_the_ffi() {
    let fonts = valo_fonts_new();
    let bytes = fira_sans_bytes();
    let face = unsafe { valo_fonts_add(fonts, bytes.as_ptr(), bytes.len()) };
    assert!(face >= 0);
    unsafe { valo_fonts_add_fallback(fonts, face) };

    let system_fonts = valo_system_fonts_new();
    let build_and_layout = |fonts| {
        let family = "Helvetica";
        let mut style = white_text_style();
        style.families_utf8 = family.as_ptr();
        style.families_length = family.len();
        let builder = unsafe { valo_paragraph_builder_new(fonts, plain_paragraph_style()) };
        let text = "Hello 中文";
        unsafe { valo_paragraph_builder_add_text(builder, text.as_ptr(), text.len(), style) };
        let paragraph = unsafe { valo_paragraph_builder_build(builder, fonts as *mut _) };
        unsafe { valo_paragraph_layout(paragraph, 300.0) };
        paragraph
    };

    let first = build_and_layout(fonts);
    unsafe {
        let families_length = valo_paragraph_demand_families(first, std::ptr::null_mut(), 0);
        let mut families = vec![0u8; families_length];
        valo_paragraph_demand_families(first, families.as_mut_ptr(), families.len());
        assert_eq!(String::from_utf8(families).as_deref(), Ok("Helvetica"));

        let codepoint_count = valo_paragraph_demand_codepoints(first, std::ptr::null_mut(), 0);
        let mut codepoints = vec![0u32; codepoint_count];
        valo_paragraph_demand_codepoints(first, codepoints.as_mut_ptr(), codepoints.len());
        assert!(codepoints.contains(&('中' as u32)));

        let satisfied = valo_fonts_satisfy_demand(fonts, system_fonts);
        valo_paragraph_dispose(first);
        if !satisfied {
            eprintln!("SKIP: this machine's installed fonts cannot answer");
            valo_system_fonts_dispose(system_fonts);
            valo_fonts_dispose(fonts);
            return;
        }

        let second = build_and_layout(fonts);
        assert_eq!(
            valo_paragraph_demand_families(second, std::ptr::null_mut(), 0),
            0,
            "nothing demanded after satisfaction"
        );
        assert_eq!(
            valo_paragraph_demand_codepoints(second, std::ptr::null_mut(), 0),
            0
        );
        assert!(
            !valo_fonts_satisfy_demand(fonts, system_fonts),
            "an empty demand grows nothing"
        );
        valo_paragraph_dispose(second);
        valo_system_fonts_dispose(system_fonts);
        valo_fonts_dispose(fonts);
    }
}

/// Every function must shrug at null handles — the header's promise.
#[test]
fn null_handles_never_crash() {
    unsafe {
        valo_context_dispose(std::ptr::null_mut());
        valo_context_resize(std::ptr::null_mut(), 10, 10);
        assert!(valo_context_metal_device(std::ptr::null_mut()).is_null());
        valo_context_wait_for_gpu(std::ptr::null_mut());
        assert!(valo_context_import_metal_texture(
            std::ptr::null_mut(),
            std::ptr::null_mut(),
            1,
            1,
            0
        )
        .is_null());
        assert!(!valo_context_render_to_metal_texture(
            std::ptr::null_mut(),
            std::ptr::null(),
            CLEAR,
            std::ptr::null_mut(),
            1,
            1,
            0
        ));
        assert!(!valo_context_render(
            std::ptr::null_mut(),
            std::ptr::null(),
            CLEAR
        ));
        assert!(!valo_context_render_to_pixels(
            std::ptr::null_mut(),
            std::ptr::null(),
            CLEAR,
            1,
            1,
            std::ptr::null_mut()
        ));
        assert!(valo_context_create_image(std::ptr::null_mut(), 1, 1, std::ptr::null()).is_null());
        valo_image_dispose(std::ptr::null_mut());

        assert!(valo_builder_build(std::ptr::null_mut()).is_null());
        valo_builder_dispose(std::ptr::null_mut());
        valo_display_list_dispose(std::ptr::null_mut());
        valo_builder_draw_rect(
            std::ptr::null_mut(),
            rect(0.0, 0.0, 1.0, 1.0),
            fill(1.0, 0.0, 0.0),
        );
        valo_builder_draw_path(
            std::ptr::null_mut(),
            std::ptr::null_mut(),
            0,
            fill(1.0, 0.0, 0.0),
        );
        valo_builder_transform_matrix(std::ptr::null_mut(), std::ptr::null());
        valo_builder_draw_paragraph(std::ptr::null_mut(), std::ptr::null(), 0.0, 0.0);
        valo_builder_draw_paragraph_with(
            std::ptr::null_mut(),
            std::ptr::null(),
            0.0,
            0.0,
            fill(1.0, 0.0, 0.0),
        );

        valo_color_filter_dispose(std::ptr::null_mut());
        assert!(valo_color_filter_matrix(std::ptr::null()).is_null());

        valo_path_dispose(std::ptr::null_mut());
        valo_path_line_to(std::ptr::null_mut(), 1.0, 1.0);
        valo_path_add_arc(
            std::ptr::null_mut(),
            ValoPoint { x: 0.0, y: 0.0 },
            4.0,
            0.0,
            1.0,
        );
        valo_path_add_ellipse(
            std::ptr::null_mut(),
            ValoPoint { x: 0.0, y: 0.0 },
            4.0,
            2.0,
            0.0,
            0.0,
            1.0,
        );
        valo_path_arc_to(
            std::ptr::null_mut(),
            ValoPoint { x: 1.0, y: 0.0 },
            ValoPoint { x: 1.0, y: 1.0 },
            1.0,
        );
        assert!(!valo_path_contains(
            std::ptr::null_mut(),
            ValoPoint { x: 0.0, y: 0.0 },
            0
        ));

        valo_fonts_dispose(std::ptr::null_mut());
        assert_eq!(
            valo_fonts_add(std::ptr::null_mut(), std::ptr::null(), 0),
            -1
        );
        assert_eq!(
            valo_fonts_add_instances(std::ptr::null_mut(), std::ptr::null(), 0, true),
            0
        );
        valo_system_fonts_dispose(std::ptr::null_mut());
        assert_eq!(valo_system_fonts_face_count(std::ptr::null()), 0);
        assert_eq!(
            valo_fonts_add_system_family(
                std::ptr::null_mut(),
                std::ptr::null_mut(),
                std::ptr::null(),
                0
            ),
            0
        );
        assert!(!valo_fonts_satisfy_demand(
            std::ptr::null_mut(),
            std::ptr::null_mut()
        ));
        assert_eq!(
            valo_paragraph_demand_families(std::ptr::null(), std::ptr::null_mut(), 0),
            0
        );
        assert_eq!(
            valo_paragraph_demand_codepoints(std::ptr::null(), std::ptr::null_mut(), 0),
            0
        );
        assert!(valo_paragraph_builder_new(
            std::ptr::null(),
            ValoParagraphStyle {
                align: 0,
                max_lines: 0,
                ellipsis_utf8: std::ptr::null(),
                ellipsis_length: 0,
            }
        )
        .is_null());
        assert!(valo_paragraph_builder_build(std::ptr::null_mut(), std::ptr::null_mut()).is_null());
        valo_paragraph_dispose(std::ptr::null_mut());
        valo_paragraph_layout(std::ptr::null_mut(), 100.0);
        assert_eq!(valo_paragraph_width(std::ptr::null()), 0.0);
    }
}

/// The hand-kept header must declare EXACTLY the exported symbol set.
#[test]
fn header_declares_exactly_the_exported_symbols() {
    let root = std::path::Path::new(env!("CARGO_MANIFEST_DIR"));
    let header = std::fs::read_to_string(root.join("include/valo.h")).expect("include/valo.h");

    // Every module in src, not a hand-kept list — a new module must not be
    // able to escape this check by being forgotten here.
    let mut exported = std::collections::BTreeSet::new();
    for entry in std::fs::read_dir(root.join("src")).expect("src") {
        let path = entry.expect("dir entry").path();
        if path.extension().is_some_and(|extension| extension == "rs") {
            let code = std::fs::read_to_string(&path).expect("module source");
            collect_exported_functions(&code, &mut exported);
        }
    }
    assert!(!exported.is_empty());

    let mut declared = std::collections::BTreeSet::new();
    for name in exported.iter() {
        // A declaration is the name followed by an open paren, outside the
        // comment prose (prose never uses parens after a symbol name).
        if header.contains(&format!("{name}(")) {
            declared.insert(name.clone());
        }
    }
    let missing: Vec<_> = exported.difference(&declared).collect();
    assert!(
        missing.is_empty(),
        "header is missing declarations for: {missing:?}"
    );

    // And nothing phantom: every valo_-prefixed call the header declares
    // must be exported.
    for captured in header.split(|c: char| !(c.is_ascii_alphanumeric() || c == '_')) {
        if captured.starts_with("valo_") && header.contains(&format!("{captured}(")) {
            assert!(
                exported.contains(captured),
                "header declares {captured} but the crate does not export it"
            );
        }
    }
}

fn collect_exported_functions(code: &str, out: &mut std::collections::BTreeSet<String>) {
    // Outside comments and imports, every `valo_*` identifier in the
    // sources IS an exported function: plain `extern "C" fn` definitions
    // plus the names handed to the op macros (which paste them into
    // `extern "C" fn $name`). Source files never CALL each other's
    // exports, so occurrence == definition. (`use` lines are excluded —
    // crate names share the `valo_` prefix.)
    for line in code.lines() {
        if line.trim_start().starts_with("use ") {
            continue;
        }
        let code_only = line.split("//").next().unwrap_or("");
        for token in code_only.split(|c: char| !(c.is_ascii_alphanumeric() || c == '_')) {
            if token.starts_with("valo_") {
                out.insert(token.to_owned());
            }
        }
    }
}
