//! `Path::combine` on the calls Flutter's framework makes with
//! `Path.combine`, checked against Skia's own answers, which
//! valo-pathops' reference build recorded in its `flutter.txt` fixture.
//!
//! Two checks per case. Given Skia's inputs exactly, `combine` must give
//! Skia's result verb for verb and bit for bit, which tests the way a valo
//! path goes to the port and back. Given the same shapes built with valo's
//! own helpers (which start their outlines elsewhere than Skia's), it must
//! cover the same area as Skia's result.

use std::sync::Arc;

use valo_geometry::{
    FillRule, Path, PathBuilder, PathElement, PathOperation, PathOperationError, Point, Rect,
};

/// `Recorded` is one fixture case: Skia's two inputs and its result.
struct Recorded {
    inputs: Vec<Path>,
    result: Path,
}

fn recorded(name: &str) -> Recorded {
    let path = concat!(
        env!("CARGO_MANIFEST_DIR"),
        "/../valo-pathops/tests/fixtures/flutter.txt"
    );
    let text = std::fs::read_to_string(path).expect("the flutter fixture is readable");
    let mut lines = text
        .lines()
        .skip_while(|line| *line != format!("case {name}"))
        .skip(1)
        .take_while(|line| !line.starts_with("case "));
    let mut inputs = Vec::new();
    let mut result = None;
    for line in lines.by_ref() {
        if let Some(fields) = line.strip_prefix("input ") {
            inputs.push(parse_path(fields));
        } else if let Some(fields) = line.strip_prefix("result ") {
            result = Some(parse_path(fields));
        }
    }
    Recorded {
        inputs,
        result: result.unwrap_or_else(|| panic!("case {name} has a result")),
    }
}

/// `parse_path` reads a fixture path, `<fill> <verbs> <x:y,...> <weights|->`,
/// with every float written as its bits in hex.
fn parse_path(fields: &str) -> Path {
    let fields: Vec<&str> = fields.split(' ').collect();
    let [_fill, verbs, points, weights] = fields[..] else {
        panic!("a fixture path has four fields: {fields:?}");
    };
    let float = |hex: &str| f32::from_bits(u32::from_str_radix(hex, 16).expect("hex bits"));
    let mut points = points.split(',').map(|point| {
        let (x, y) = point.split_once(':').expect("a point is x:y");
        Point::new(float(x), float(y))
    });
    let mut weights = weights
        .split(',')
        .filter(|weight| *weight != "-")
        .map(float);
    let mut next = || points.next().expect("a point per verb point");
    let mut builder = PathBuilder::new();
    for verb in verbs.chars() {
        match verb {
            'M' => builder.move_to(next()),
            'L' => builder.line_to(next()),
            'Q' => {
                let control = next();
                builder.quad_to(control, next())
            }
            'K' => {
                let control = next();
                let weight = weights.next().expect("a weight per conic");
                builder.conic_to(control, next(), weight)
            }
            'C' => {
                let (control1, control2) = (next(), next());
                builder.cubic_to(control1, control2, next())
            }
            'Z' => builder.close(),
            other => panic!("unknown verb {other}"),
        };
    }
    Arc::unwrap_or_clone(builder.build())
}

fn elements(path: &Path) -> Vec<PathElement> {
    path.elements().collect()
}

fn difference(minuend: &Path, subtrahend: &Path) -> Result<Arc<Path>, PathOperationError> {
    minuend.combine(
        FillRule::NonZero,
        PathOperation::Difference,
        subtrahend,
        FillRule::NonZero,
    )
}

/// `assert_matches_skia` checks that Skia's own inputs give Skia's result
/// exactly: the same elements, compared bit for bit.
fn assert_matches_skia(name: &str) {
    let case = recorded(name);
    let combined = difference(&case.inputs[0], &case.inputs[1]).expect("Skia succeeded");
    let bits = |path: &Path| format!("{:?}", elements(path));
    assert_eq!(bits(&combined), bits(&case.result), "{name}");
    assert!(combined
        .elements()
        .zip(case.result.elements())
        .all(|(ours, skias)| same_bits(ours, skias)));
}

fn same_bits(a: PathElement, b: PathElement) -> bool {
    let points = |element: PathElement| -> Vec<u32> {
        let points: Vec<Point> = match element {
            PathElement::MoveTo(p) | PathElement::LineTo(p) => vec![p],
            PathElement::QuadTo(c, p) => vec![c, p],
            PathElement::ConicTo(c, p, weight) => vec![c, p, Point::new(weight, 0.0)],
            PathElement::CubicTo(c1, c2, p) => vec![c1, c2, p],
            PathElement::Close => vec![],
        };
        points
            .iter()
            .flat_map(|p| [p.x.to_bits(), p.y.to_bits()])
            .collect()
    };
    std::mem::discriminant(&a) == std::mem::discriminant(&b) && points(a) == points(b)
}

/// `assert_same_area` checks that `ours`, filled even-odd as `combine`'s
/// results are, covers what Skia's result covers, on a grid of points
/// offset so none lies on the shapes' straight edges.
fn assert_same_area(name: &str, ours: &Path, skias: &Path) {
    let bounds = skias.bounds();
    let mut differing = Vec::new();
    let mut y = bounds.y - 2.0 + 0.13;
    while y < bounds.bottom() + 2.0 {
        let mut x = bounds.x - 2.0 + 0.17;
        while x < bounds.right() + 2.0 {
            let point = Point::new(x, y);
            if ours.contains(point, FillRule::EvenOdd) != skias.contains(point, FillRule::EvenOdd) {
                differing.push(point);
            }
            x += 0.5;
        }
        y += 0.5;
    }
    assert!(
        differing.is_empty(),
        "{name}: {} sample points differ, first {:?}",
        differing.len(),
        differing.first()
    );
}

fn built(build: impl FnOnce(&mut PathBuilder)) -> Arc<Path> {
    let mut builder = PathBuilder::new();
    build(&mut builder);
    builder.build()
}

#[test]
fn label_gap_matches_skia() {
    assert_matches_skia("label-gap");
}

#[test]
fn circle_notch_matches_skia() {
    assert_matches_skia("circle-notch");
}

#[test]
fn rounded_rectangle_notch_matches_skia() {
    assert_matches_skia("rounded-rectangle-notch");
}

#[test]
fn label_gap_from_valo_shapes_covers_skias_area() {
    let outline = built(|b| {
        b.rrect(Rect::from_ltrb(0.5, 0.5, 299.5, 55.5), 4.0);
    });
    let gap = built(|b| {
        b.rect(Rect::from_ltrb(12.0, -0.5, 52.0, 1.5));
    });
    let combined = difference(&outline, &gap).expect("the gap is cut");
    assert_same_area("label-gap", &combined, &recorded("label-gap").result);
}

#[test]
fn circle_notch_from_valo_shapes_covers_skias_area() {
    let bar = built(|b| {
        b.rect(Rect::from_ltrb(0.0, 0.0, 360.0, 56.0));
    });
    let button = built(|b| {
        b.circle((180.0, 0.0), 32.0);
    });
    let combined = difference(&bar, &button).expect("the notch is cut");
    assert_same_area("circle-notch", &combined, &recorded("circle-notch").result);
}

#[test]
fn rounded_rectangle_notch_from_valo_shapes_covers_skias_area() {
    let bar = built(|b| {
        b.rect(Rect::from_ltrb(0.0, 0.0, 360.0, 56.0));
    });
    let button = built(|b| {
        b.rrect(Rect::from_ltrb(148.0, -32.0, 212.0, 32.0), 16.0);
    });
    let combined = difference(&bar, &button).expect("the notch is cut");
    assert_same_area(
        "rounded-rectangle-notch",
        &combined,
        &recorded("rounded-rectangle-notch").result,
    );
}

#[test]
fn every_operation_combines_two_overlapping_squares() {
    let left = built(|b| {
        b.rect(Rect::from_ltrb(0.0, 0.0, 20.0, 20.0));
    });
    let right = built(|b| {
        b.rect(Rect::from_ltrb(10.0, 0.0, 30.0, 20.0));
    });
    let inside = |operation: PathOperation, x: f32| {
        left.combine(FillRule::NonZero, operation, &right, FillRule::NonZero)
            .expect("squares combine")
            .contains(Point::new(x, 10.0), FillRule::EvenOdd)
    };
    let cases = [
        (PathOperation::Difference, [true, false, false]),
        (PathOperation::Intersect, [false, true, false]),
        (PathOperation::Union, [true, true, true]),
        (PathOperation::Xor, [true, false, true]),
        (PathOperation::ReverseDifference, [false, false, true]),
    ];
    for (operation, expected) in cases {
        let found = [5.0, 15.0, 25.0].map(|x| inside(operation, x));
        assert_eq!(found, expected, "{operation:?}");
    }
}
