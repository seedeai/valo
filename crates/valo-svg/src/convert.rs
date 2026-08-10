//! usvg leaf types → valo vocabulary: path geometry, transforms, paints.
//! Pure mappings; a paint that cannot draw at all returns its [`Gap`] tag
//! (the caller skips it and reports), while approximations that still
//! draw record their tag on [`Missing`] directly.

use std::sync::Arc;

use usvg::tiny_skia_path;
use valo_dl::{FocalCircle, GradientStop, Paint, PaintStyle, Shader, SpreadMode};
use valo_geometry::{Cap, Color, Dash, FillRule, Join, Matrix, Path, PathBuilder, Point, Stroke};

use crate::translate::Missing;

/// A feature tag for a paint that cannot be drawn.
pub(crate) type Gap = &'static str;

/// Path data with `t` baked into the points — clips need this because valo
/// clips persist to the enclosing restore, so a transform can't scope them.
pub(crate) fn path(data: &tiny_skia_path::Path, t: usvg::Transform) -> Arc<Path> {
    use tiny_skia_path::PathSegment as S;
    let m = transform(t);
    let p = |pt: tiny_skia_path::Point| map(&m, pt.x, pt.y);
    let mut b = PathBuilder::new();
    for seg in data.segments() {
        match seg {
            S::MoveTo(a) => b.move_to(p(a)),
            S::LineTo(a) => b.line_to(p(a)),
            S::QuadTo(c, a) => b.quad_to(p(c), p(a)),
            S::CubicTo(c0, c1, a) => b.cubic_to(p(c0), p(c1), p(a)),
            S::Close => b.close(),
        };
    }
    b.build()
}

pub(crate) fn transform(t: usvg::Transform) -> Matrix {
    Matrix::from_affine(t.sx, t.ky, t.kx, t.sy, t.tx, t.ty)
}

pub(crate) fn fill_rule(rule: usvg::FillRule) -> FillRule {
    match rule {
        usvg::FillRule::NonZero => FillRule::NonZero,
        usvg::FillRule::EvenOdd => FillRule::EvenOdd,
    }
}

pub(crate) fn blend_mode(mode: usvg::BlendMode) -> valo_dl::BlendMode {
    use valo_dl::BlendMode as B;
    match mode {
        usvg::BlendMode::Normal => B::SrcOver,
        usvg::BlendMode::Multiply => B::Multiply,
        usvg::BlendMode::Screen => B::Screen,
        usvg::BlendMode::Overlay => B::Overlay,
        usvg::BlendMode::Darken => B::Darken,
        usvg::BlendMode::Lighten => B::Lighten,
        usvg::BlendMode::ColorDodge => B::ColorDodge,
        usvg::BlendMode::ColorBurn => B::ColorBurn,
        usvg::BlendMode::HardLight => B::HardLight,
        usvg::BlendMode::SoftLight => B::SoftLight,
        usvg::BlendMode::Difference => B::Difference,
        usvg::BlendMode::Exclusion => B::Exclusion,
        usvg::BlendMode::Hue => B::Hue,
        usvg::BlendMode::Saturation => B::Saturation,
        usvg::BlendMode::Color => B::Color,
        usvg::BlendMode::Luminosity => B::Luminosity,
    }
}

pub(crate) fn fill_paint(f: &usvg::Fill, m: &mut Missing) -> Result<(Paint, FillRule), Gap> {
    let paint = paint(f.paint(), f.opacity().get(), PaintStyle::Fill, m)?;
    Ok((paint, fill_rule(f.rule())))
}

pub(crate) fn stroke_paint(s: &usvg::Stroke, m: &mut Missing) -> Result<Paint, Gap> {
    paint(
        s.paint(),
        s.opacity().get(),
        PaintStyle::Stroke(stroke(s)),
        m,
    )
}

pub(crate) fn stroke_geometry(s: &usvg::Stroke) -> Stroke {
    stroke(s)
}

fn stroke(s: &usvg::Stroke) -> Stroke {
    Stroke {
        width: s.width().get(),
        cap: match s.linecap() {
            usvg::LineCap::Butt => Cap::Butt,
            usvg::LineCap::Round => Cap::Round,
            usvg::LineCap::Square => Cap::Square,
        },
        join: match s.linejoin() {
            // miter-clip renders as plain miter (Skia's mapping too).
            usvg::LineJoin::Miter | usvg::LineJoin::MiterClip => Join::Miter,
            usvg::LineJoin::Round => Join::Round,
            usvg::LineJoin::Bevel => Join::Bevel,
        },
        miter_limit: s.miterlimit().get(),
        dash: s.dasharray().map(|d| Dash {
            intervals: d.to_vec(),
            phase: s.dashoffset(),
        }),
    }
}

/// `opacity` is the fill-opacity/stroke-opacity of the referencing paint:
/// solid colors carry it in their alpha; gradients ride it through the
/// tint color (valo multiplies shader output by `Paint::color`).
fn paint(p: &usvg::Paint, opacity: f32, style: PaintStyle, m: &mut Missing) -> Result<Paint, Gap> {
    let mut out = Paint {
        style,
        ..Paint::default()
    };
    match p {
        usvg::Paint::Color(c) => out.color = color(*c, opacity),
        usvg::Paint::LinearGradient(g) => {
            out.shader = Some(linear(g, m)?);
            out.color = Color::rgba(1.0, 1.0, 1.0, opacity);
        }
        usvg::Paint::RadialGradient(g) => {
            out.shader = Some(radial(g, m)?);
            out.color = Color::rgba(1.0, 1.0, 1.0, opacity);
        }
        // Pattern FILLS are intercepted upstream (translate records tile
        // embeds); reaching here means a pattern somewhere the translator
        // can't express — the caller skips this paint.
        usvg::Paint::Pattern(_) => return Err("pattern-paint"),
    }
    Ok(out)
}

/// feDropShadow's flood color with its opacity.
pub(crate) fn shadow_color(c: usvg::Color, opacity: f32) -> Color {
    color(c, opacity)
}

fn color(c: usvg::Color, a: f32) -> Color {
    Color {
        a,
        ..Color::from_rgba8(c.red, c.green, c.blue, 255)
    }
}

/// gradientTransform rides the shader's LOCAL MATRIX (Skia's shape): the
/// fragment evaluates in gradient space, so every invertible affine —
/// skews, the non-square objectBoundingBox default, elliptical radials —
/// is exact. Control points pass through RAW.
fn linear(g: &usvg::LinearGradient, m: &mut Missing) -> Result<Shader, Gap> {
    let local = invertible(g.transform())?;
    Ok(Shader::Linear {
        start: (g.x1(), g.y1()).into(),
        end: (g.x2(), g.y2()).into(),
        stops: stops(g.stops(), m)?,
        spread: spread(g.spread_method()),
        local,
    })
}

fn radial(g: &usvg::RadialGradient, m: &mut Missing) -> Result<Shader, Gap> {
    Ok(Shader::Radial {
        center: (g.cx(), g.cy()).into(),
        radius: g.r().get(),
        stops: stops(g.stops(), m)?,
        spread: spread(g.spread_method()),
        focus: focus(g),
        local: invertible(g.transform())?,
    })
}

/// The focal point, clamped INSIDE the circle when the document puts it
/// on or past the rim — the spec's UA behavior (Skia clamps the same
/// way), and the domain valo's focal solve needs.
fn focus(g: &usvg::RadialGradient) -> Option<FocalCircle> {
    let (dx, dy) = (g.fx() - g.cx(), g.fy() - g.cy());
    let d = (dx * dx + dy * dy).sqrt();
    if d < 1e-4 {
        return None; // centered — the classic gradient
    }
    let r = g.r().get();
    let k = if d >= r { r * 0.999 / d } else { 1.0 };
    Some(FocalCircle::point(
        (g.cx() + dx * k, g.cy() + dy * k).into(),
    ))
}

fn invertible(t: usvg::Transform) -> Result<Matrix, Gap> {
    let out = transform(t);
    // A collapsed gradient space renders nothing sensible; SVG says the
    // element is not rendered — the caller skips this paint.
    if out.invert().is_none() {
        return Err("degenerate-gradient");
    }
    Ok(out)
}

fn spread(method: usvg::SpreadMethod) -> SpreadMode {
    match method {
        usvg::SpreadMethod::Pad => SpreadMode::Pad,
        usvg::SpreadMethod::Repeat => SpreadMode::Repeat,
        usvg::SpreadMethod::Reflect => SpreadMode::Reflect,
    }
}

/// Any stop count renders: ≤[`MAX_GRADIENT_STOPS`] rides valo's analytic
/// uniform ramp, beyond that the renderer bakes a texture ramp (Impeller's
/// path — resolution capped at 1024 texels, so lists past
/// ~1024 stops clamp to endpoints and tag).
fn stops(stops: &[usvg::Stop], m: &mut Missing) -> Result<Vec<GradientStop>, Gap> {
    if stops.is_empty() {
        return Err("gradient-stops");
    }
    const SANITY: usize = 1024;
    let mut picked: Vec<&usvg::Stop> = stops.iter().collect();
    if picked.len() > SANITY {
        m.add("gradient-stops");
        picked = stops[..SANITY - 1].iter().chain(stops.last()).collect();
    }
    Ok(picked
        .into_iter()
        .map(|s| GradientStop {
            offset: s.offset().get(),
            color: color(s.color(), s.opacity().get()),
        })
        .collect())
}

fn map(t: &Matrix, x: f32, y: f32) -> Point {
    t.map_point(Point::new(x, y))
}
