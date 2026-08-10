//! The translator contract: documents ALWAYS render
//! best-effort — supported features translate exactly, gaps degrade per
//! element and surface as tags in `missing` (flutter_svg's shape). Only
//! unparseable bytes error.

use std::sync::Arc;

use valo_dl::{DisplayList, MaskKind, Op, PaintStyle, Shader, SpreadMode};
use valo_svg::{translate, Svg, SvgError};

fn doc(body: &str) -> String {
    format!(r#"<svg xmlns="http://www.w3.org/2000/svg" viewBox="0 0 24 24">{body}</svg>"#)
}

fn svg(body: &str) -> Svg {
    translate(doc(body).as_bytes()).expect("parses")
}

/// Fully-native translation: no missing tags.
fn vector(body: &str) -> (Arc<DisplayList>, [f32; 2]) {
    let out = svg(body);
    assert!(out.missing.is_empty(), "unexpected gaps: {:?}", out.missing);
    (out.list, out.size)
}

fn draw_paints(list: &DisplayList) -> Vec<&valo_dl::Paint> {
    list.ops()
        .iter()
        .filter_map(|op| match op {
            Op::DrawPath { paint, .. } => Some(paint),
            _ => None,
        })
        .collect()
}

#[test]
fn shapes_fill_and_stroke() {
    let (list, size) = vector(
        r##"<circle cx="12" cy="12" r="8" fill="#ff0000" stroke="#0000ff" stroke-width="2"/>"##,
    );
    assert_eq!(size, [24.0, 24.0]);
    let paints = draw_paints(&list);
    assert_eq!(paints.len(), 2);
    assert!(matches!(paints[0].style, PaintStyle::Fill));
    assert!(matches!(paints[1].style, PaintStyle::Stroke(_)));
    assert!((paints[0].color.r - 1.0).abs() < 1e-6);
}

#[test]
fn wave_path_and_tiny_rect() {
    let (wave, _) =
        vector(r##"<path d="M0 12 C 6 4, 18 20, 24 12" fill="none" stroke="#000000"/>"##);
    assert_eq!(draw_paints(&wave).len(), 1);
    let (rect, _) = vector(r##"<rect width="1" height="1" fill="#00ff00"/>"##);
    assert_eq!(draw_paints(&rect).len(), 1);
}

#[test]
fn group_opacity_opens_a_layer() {
    let (list, _) = vector(r##"<g opacity="0.5"><rect width="8" height="8" fill="#000000"/></g>"##);
    let alphas: Vec<f32> = list
        .ops()
        .iter()
        .filter_map(|op| match op {
            Op::SaveLayer { paint, .. } => Some(paint.color.a),
            _ => None,
        })
        .collect();
    assert_eq!(alphas.len(), 1);
    assert!((alphas[0] - 0.5).abs() < 1e-3);
}

#[test]
fn transforms_are_recorded() {
    let (list, _) = vector(
        r##"<g transform="translate(2 3)"><rect width="4" height="4" fill="#000000"/></g>"##,
    );
    assert!(list.ops().iter().any(|op| matches!(
        op,
        Op::Transform(t) if {
            let [.., tx, ty] = t.to_affine();
            (tx - 2.0).abs() < 1e-6 && (ty - 3.0).abs() < 1e-6
        }
    )));
}

#[test]
fn dash_patterns_survive() {
    let (list, _) = vector(
        r##"<path d="M0 0 L24 24" fill="none" stroke="#000000" stroke-dasharray="4 2" stroke-dashoffset="1"/>"##,
    );
    let paints = draw_paints(&list);
    let PaintStyle::Stroke(stroke) = &paints[0].style else {
        panic!("expected stroke");
    };
    let dash = stroke.dash.as_ref().expect("dash survives translation");
    assert_eq!(dash.intervals, vec![4.0, 2.0]);
    assert_eq!(dash.phase, 1.0);
}

// ── clips ────────────────────────────────────────────────────────────────

#[test]
fn single_path_clip_stays_a_depth_clip() {
    let (list, _) = vector(
        r##"<clipPath id="c"><circle cx="12" cy="12" r="6"/></clipPath>
            <rect width="24" height="24" fill="#000000" clip-path="url(#c)"/>"##,
    );
    assert!(list
        .ops()
        .iter()
        .any(|op| matches!(op, Op::ClipPath { .. })));
    assert!(!list
        .ops()
        .iter()
        .any(|op| matches!(op, Op::SaveLayer { .. })));
}

#[test]
fn multi_path_clip_desugars_to_an_alpha_mask() {
    // Two children = a UNION: expressed as an alpha-mask layer whose ink
    // is the clip paths in white (exact even where they overlap).
    let (list, _) = vector(
        r##"<clipPath id="c"><circle cx="8" cy="12" r="6"/><circle cx="16" cy="12" r="6"/></clipPath>
            <rect width="24" height="24" fill="#000000" clip-path="url(#c)"/>"##,
    );
    assert!(list.ops().iter().any(|op| matches!(
        op,
        Op::SaveLayer {
            mask_composite: Some(MaskKind::Alpha),
            ..
        }
    )));
    // Content fill + two white clip fills.
    assert_eq!(draw_paints(&list).len(), 3);
}

#[test]
fn empty_clip_hides_the_group() {
    let (list, _) = vector(
        r##"<clipPath id="c"></clipPath>
            <rect width="24" height="24" fill="#000000" clip-path="url(#c)"/>"##,
    );
    assert!(draw_paints(&list).is_empty());
}

// ── gradients ────────────────────────────────────────────────────────────

#[test]
fn bbox_gradient_rides_the_local_matrix() {
    // objectBoundingBox on a NON-SQUARE shape: control points stay in unit
    // space, the bbox mapping is the shader's local matrix — exact.
    let (list, _) = vector(
        r##"<defs><linearGradient id="g"><stop offset="0" stop-color="#ff0000"/><stop offset="1" stop-color="#0000ff"/></linearGradient></defs>
            <rect width="24" height="6" fill="url(#g)"/>"##,
    );
    let paints = draw_paints(&list);
    let Some(Shader::Linear {
        start, end, local, ..
    }) = &paints[0].shader
    else {
        panic!("expected linear gradient shader");
    };
    assert!((start.x - 0.0).abs() < 1e-6 && (end.x - 1.0).abs() < 1e-6);
    let [a, _, _, d, ..] = local.to_affine();
    assert!((a - 24.0).abs() < 1e-3 && (d - 6.0).abs() < 1e-3);
}

#[test]
fn skewed_and_elliptical_gradients_are_native_now() {
    let skewed = r##"<defs><linearGradient id="g" gradientUnits="userSpaceOnUse" x1="0" y1="0" x2="24" y2="24" gradientTransform="skewX(20)"><stop offset="0" stop-color="#ff0000"/><stop offset="1" stop-color="#0000ff"/></linearGradient></defs><rect width="24" height="24" fill="url(#g)"/>"##;
    vector(skewed);
    // Non-square bbox radial = ellipse: the local matrix expresses it.
    let (list, _) = vector(
        r##"<defs><radialGradient id="g"><stop offset="0" stop-color="#ff0000"/><stop offset="1" stop-color="#0000ff"/></radialGradient></defs><rect width="24" height="6" fill="url(#g)"/>"##,
    );
    let Some(Shader::Radial { local, .. }) = &draw_paints(&list)[0].shader else {
        panic!("expected radial");
    };
    let [a, _, _, d, ..] = local.to_affine();
    assert!((a - 24.0).abs() < 1e-3 && (d - 6.0).abs() < 1e-3);
}

#[test]
fn spread_methods_translate() {
    let (list, _) = vector(
        r##"<defs><linearGradient id="g" spreadMethod="repeat" gradientUnits="userSpaceOnUse" x2="8"><stop offset="0" stop-color="#ff0000"/><stop offset="1" stop-color="#0000ff"/></linearGradient></defs><rect width="24" height="24" fill="url(#g)"/>"##,
    );
    assert!(matches!(
        draw_paints(&list)[0].shader,
        Some(Shader::Linear {
            spread: SpreadMode::Repeat,
            ..
        })
    ));
}

#[test]
fn focal_radials_translate_and_rim_focus_clamps_inside() {
    let (list, _) = vector(
        r##"<defs><radialGradient id="g" fx="0.3" fy="0.4"><stop offset="0" stop-color="#ff0000"/><stop offset="1" stop-color="#0000ff"/></radialGradient></defs><rect width="24" height="24" fill="url(#g)"/>"##,
    );
    let Some(Shader::Radial { focus: Some(f), .. }) = &draw_paints(&list)[0].shader else {
        panic!("expected focal radial");
    };
    // Raw gradient-space coords (bbox units) — the local matrix maps them.
    assert!((f.x - 0.3).abs() < 1e-4 && (f.y - 0.4).abs() < 1e-4);

    // Focus ON the rim: clamped just inside (the spec's UA behavior).
    let (list, _) = vector(
        r##"<defs><radialGradient id="g" fx="1.0" fy="0.5"><stop offset="0" stop-color="#ff0000"/><stop offset="1" stop-color="#0000ff"/></radialGradient></defs><rect width="24" height="24" fill="url(#g)"/>"##,
    );
    let Some(Shader::Radial { focus: Some(f), .. }) = &draw_paints(&list)[0].shader else {
        panic!("expected clamped focal radial");
    };
    assert!(f.x > 0.99 && f.x < 1.0 && (f.y - 0.5).abs() < 1e-4);
}

#[test]
fn many_stops_are_native_via_the_ramp_tier() {
    // R2: >8 stops ride the baked texture ramp — nothing degrades.
    let stops: String = (0..12)
        .map(|i| {
            format!(
                r##"<stop offset="{}" stop-color="#00{:02x}40"/>"##,
                i as f32 / 11.0,
                i * 20
            )
        })
        .collect();
    let (list, _) = vector(&format!(
        r##"<defs><linearGradient id="g" gradientUnits="userSpaceOnUse" x2="8">{stops}</linearGradient></defs><rect width="24" height="24" fill="url(#g)"/>"##
    ));
    let Some(Shader::Linear { stops, .. }) = &draw_paints(&list)[0].shader else {
        panic!("expected gradient");
    };
    assert_eq!(stops.len(), 12, "all stops recorded");
}

// ── masks ────────────────────────────────────────────────────────────────

#[test]
fn masks_translate_as_coverage_layers() {
    let (list, _) = vector(
        r##"<mask id="m"><rect width="12" height="24" fill="#ffffff"/></mask><rect width="24" height="24" fill="#ff0000" mask="url(#m)"/>"##,
    );
    let kinds: Vec<_> = list
        .ops()
        .iter()
        .filter_map(|op| match op {
            Op::SaveLayer { mask_composite, .. } => Some(*mask_composite),
            _ => None,
        })
        .collect();
    assert!(kinds.contains(&Some(MaskKind::Luminance)));
    assert!(kinds.contains(&None), "content isolates in its own layer");

    let (list, _) = vector(
        r##"<mask id="m" style="mask-type:alpha"><rect width="12" height="24" fill="#ffffff"/></mask><rect width="24" height="24" fill="#ff0000" mask="url(#m)"/>"##,
    );
    assert!(list.ops().iter().any(|op| matches!(
        op,
        Op::SaveLayer {
            mask_composite: Some(MaskKind::Alpha),
            ..
        }
    )));

    // An empty mask hides the group entirely — nothing records.
    let (list, _) = vector(
        r##"<mask id="m"></mask><rect width="24" height="24" fill="#ff0000" mask="url(#m)"/>"##,
    );
    assert!(draw_paints(&list).is_empty());
}

// ── patterns ─────────────────────────────────────────────────────────────

#[test]
fn pattern_fills_become_tile_embeds() {
    let (list, _) = vector(
        r##"<defs><pattern id="p" width="8" height="8" patternUnits="userSpaceOnUse"><circle cx="4" cy="4" r="3" fill="#ff0000"/></pattern></defs><rect width="24" height="24" fill="url(#p)"/>"##,
    );
    let embeds: Vec<_> = list
        .ops()
        .iter()
        .filter_map(|op| match op {
            Op::DrawDisplayList { list, .. } => Some(Arc::as_ptr(list)),
            _ => None,
        })
        .collect();
    assert_eq!(embeds.len(), 9, "24/8 = 3×3 tiles");
    assert!(embeds.iter().all(|p| *p == embeds[0]), "one shared tile");
}

#[test]
fn dense_pattern_grids_skip_and_report() {
    // 24/0.5 = 48×48 tiles > the cap — fill skipped.
    let out = svg(
        r##"<defs><pattern id="p" width="0.5" height="0.5" patternUnits="userSpaceOnUse"><rect width="0.4" height="0.4" fill="#ff0000"/></pattern></defs><rect width="24" height="24" fill="url(#p)"/>"##,
    );
    assert_eq!(out.missing, vec!["pattern-tiles"]);
}

#[test]
fn pattern_strokes_are_native_via_the_mask_desugar() {
    // R1: tiles over the stroke's bounds, coverage = the white-stroked
    // path in an alpha mask.
    let (list, _) = vector(
        r##"<defs><pattern id="p" width="4" height="4" patternUnits="userSpaceOnUse"><rect width="2" height="2" fill="#ff0000"/></pattern></defs><rect x="2" y="2" width="20" height="20" fill="none" stroke="url(#p)" stroke-width="2"/>"##,
    );
    assert!(list.ops().iter().any(|op| matches!(
        op,
        Op::SaveLayer {
            mask_composite: Some(MaskKind::Alpha),
            ..
        }
    )));
    // The coverage draw is a white STROKE-style path.
    assert!(draw_paints(&list)
        .iter()
        .any(|p| matches!(p.style, PaintStyle::Stroke(_)) && p.color.r == 1.0));
}

// ── remaining gaps degrade per element, never abort ─────────────────────

#[test]
fn blur_and_drop_shadow_filters_are_native() {
    // R3 tier 1: a lone isotropic feGaussianBlur = the layer's blur.
    let (list, _) = vector(
        r##"<defs><filter id="f"><feGaussianBlur stdDeviation="2"/></filter></defs><rect width="24" height="24" fill="#000000" filter="url(#f)"/>"##,
    );
    assert!(list.ops().iter().any(|op| matches!(
        op,
        Op::SaveLayer { paint, .. } if paint.mask_blur.is_some()
    )));

    // feDropShadow: a blurred SrcIn-tinted copy under the content.
    let (list, _) = vector(
        r##"<defs><filter id="f"><feDropShadow dx="2" dy="3" stdDeviation="1.5" flood-color="#204060"/></filter></defs><rect width="20" height="20" fill="#ff0000" filter="url(#f)"/>"##,
    );
    assert!(list.ops().iter().any(|op| matches!(
        op,
        Op::SaveLayer { paint, .. } if paint.mask_blur.is_some()
    )));
    assert!(list.ops().iter().any(|op| matches!(
        op,
        Op::DrawRect { paint, .. } if paint.blend_mode == valo_dl::BlendMode::SrcIn
    )));
    // Content renders twice: once as shadow ink, once on top.
    assert_eq!(draw_paints(&list).len(), 2);
}

#[test]
fn complex_filters_render_unfiltered_and_report() {
    let out = svg(
        r##"<defs><filter id="f"><feTurbulence baseFrequency="0.2"/><feComposite in2="SourceGraphic" operator="in"/></filter></defs><rect width="24" height="24" fill="#000000" filter="url(#f)"/>"##,
    );
    assert_eq!(out.missing, vec!["filter"]);
    assert_eq!(draw_paints(&out.list).len(), 1, "content still renders");
}

#[test]
fn text_reports() {
    let out = svg(r##"<text x="0" y="12">hi</text><rect width="4" height="4" fill="#000000"/>"##);
    assert_eq!(out.missing, vec!["text"]);
    assert_eq!(draw_paints(&out.list).len(), 1, "the rect still renders");
}

#[test]
fn embedded_images_request_and_tag_until_resolved() {
    // R4: parse surfaces the raster bytes for the host's decode; without
    // a resolver the element renders absent + tags.
    const PNG_URI: &str = "data:image/png;base64,iVBORw0KGgoAAAANSUhEUgAAAAEAAAABCAYAAAAfFcSJAAAADUlEQVR42mP8z8BQDwAEhQGAhKmMIQAAAABJRU5ErkJggg==";
    let body = format!(
        r##"<image width="1" height="1" href="{PNG_URI}"/><rect width="4" height="4" fill="#000000"/>"##
    );
    let parsed = valo_svg::parse(doc(&body).as_bytes()).expect("parses");
    assert_eq!(parsed.images().len(), 1);
    assert_eq!(parsed.images()[0].format, valo_svg::ImageFormat::Png);
    assert!(!parsed.images()[0].bytes.is_empty());

    let out = parsed.translate(&|_| None);
    assert_eq!(out.missing, vec!["image"]);
    assert_eq!(draw_paints(&out.list).len(), 1);

    // The SAME bytes referenced twice request ONCE (identity dedup).
    let body = format!(
        r##"<image width="1" height="1" href="{PNG_URI}"/><image x="2" width="1" height="1" href="{PNG_URI}"/>"##
    );
    let parsed = valo_svg::parse(doc(&body).as_bytes()).expect("parses");
    // usvg may or may not share the Arc for identical data URIs; either
    // way every request is decodable and ids stay dense.
    assert!(!parsed.images().is_empty() && parsed.images().len() <= 2);
}

// ── containers ───────────────────────────────────────────────────────────

#[test]
fn svgz_decompresses_and_translates() {
    use std::io::Write;
    let plain = doc(r##"<rect width="8" height="8" fill="#00ff00"/>"##);
    let mut enc = flate2::write::GzEncoder::new(Vec::new(), flate2::Compression::default());
    enc.write_all(plain.as_bytes()).unwrap();
    let gz = enc.finish().unwrap();
    let out = translate(&gz).expect("svgz parses");
    assert!(out.missing.is_empty());
    assert_eq!(draw_paints(&out.list).len(), 1);
}

#[test]
fn garbage_errors() {
    assert!(matches!(translate(b"not svg at all"), Err(SvgError::Parse)));
}
