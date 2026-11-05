//! SVG's spelling of valo's values: numbers, colours, transforms, path data and `data:` URLs.

use std::fmt::{self, Write};

use base64::Engine;
use valo::{Color, Matrix, Path};
use valo_geometry::PathElement;

/// `Number` is a coordinate as SVG takes it: to a thousandth, without trailing zeros.
pub(super) struct Number(pub f32);

impl fmt::Display for Number {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write_rounded(f, self.0, 1000.0)
    }
}

/// `PreciseNumber` is a number a small error shows in, such as a matrix's scale: to a millionth.
pub(super) struct PreciseNumber(pub f32);

impl fmt::Display for PreciseNumber {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write_rounded(f, self.0, 1_000_000.0)
    }
}

fn write_rounded(f: &mut fmt::Formatter<'_>, value: f32, steps: f64) -> fmt::Result {
    let value = if value.is_finite() { value as f64 } else { 0.0 };
    let rounded = (value * steps).round() / steps;
    if rounded == 0.0 {
        return f.write_str("0");
    }
    let decimals = steps.log10() as usize;
    let text = format!("{rounded:.decimals$}");
    f.write_str(text.trim_end_matches('0').trim_end_matches('.'))
}

/// `hex` is a colour's red, green and blue as `#rrggbb`; its alpha goes in an opacity of its
/// own.
pub(super) fn hex(color: Color) -> String {
    let channel = |value: f32| (value.clamp(0.0, 1.0) * 255.0).round() as u8;
    format!(
        "#{:02x}{:02x}{:02x}",
        channel(color.r),
        channel(color.g),
        channel(color.b)
    )
}

/// `matrix` is a transform as SVG's `matrix(a b c d e f)`; `None` for one that moves nothing.
pub(super) fn matrix(m: &Matrix) -> Option<String> {
    let [a, b, c, d, e, f] = m.to_affine();
    if [a, b, c, d, e, f] == [1.0, 0.0, 0.0, 1.0, 0.0, 0.0] {
        return None;
    }
    Some(format!(
        "matrix({} {} {} {} {} {})",
        PreciseNumber(a),
        PreciseNumber(b),
        PreciseNumber(c),
        PreciseNumber(d),
        Number(e),
        Number(f)
    ))
}

/// `path_data` is a path as SVG's `d`. SVG has no conics: each becomes the cubic with the same
/// ends and tangents, exact for the quarter circles of round corners to within 0.03%.
pub(super) fn path_data(path: &Path) -> String {
    let mut d = String::new();
    let mut start = valo::Point::new(0.0, 0.0);
    let mut last = start;
    for element in path.elements() {
        let _ = match element {
            PathElement::MoveTo(p) => write!(d, "M{} {}", Number(p.x), Number(p.y)),
            PathElement::LineTo(p) => write!(d, "L{} {}", Number(p.x), Number(p.y)),
            PathElement::QuadTo(c, p) => {
                write!(
                    d,
                    "Q{} {} {} {}",
                    Number(c.x),
                    Number(c.y),
                    Number(p.x),
                    Number(p.y)
                )
            }
            PathElement::ConicTo(c, p, w) => {
                let k = 4.0 * w / (3.0 * (1.0 + w));
                write!(
                    d,
                    "C{} {} {} {} {} {}",
                    Number(last.x + (c.x - last.x) * k),
                    Number(last.y + (c.y - last.y) * k),
                    Number(p.x + (c.x - p.x) * k),
                    Number(p.y + (c.y - p.y) * k),
                    Number(p.x),
                    Number(p.y)
                )
            }
            PathElement::CubicTo(c1, c2, p) => write!(
                d,
                "C{} {} {} {} {} {}",
                Number(c1.x),
                Number(c1.y),
                Number(c2.x),
                Number(c2.y),
                Number(p.x),
                Number(p.y)
            ),
            PathElement::Close => write!(d, "Z"),
        };
        if let PathElement::MoveTo(p) = element {
            start = p;
        }
        last = match element {
            PathElement::MoveTo(p)
            | PathElement::LineTo(p)
            | PathElement::QuadTo(_, p)
            | PathElement::ConicTo(_, p, _)
            | PathElement::CubicTo(_, _, p) => p,
            PathElement::Close => start,
        };
    }
    d
}

/// `data_url` is `bytes` of the media type `kind` as a `data:` URL, which even an SVG shown as
/// an image loads.
pub(super) fn data_url(kind: &str, bytes: &[u8]) -> String {
    format!(
        "data:{kind};base64,{}",
        base64::engine::general_purpose::STANDARD.encode(bytes)
    )
}
