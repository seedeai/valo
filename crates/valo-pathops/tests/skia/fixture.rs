//! Reads the fixture files tests/reference/build.sh writes: each case of one of Skia's PathOps
//! test files, with the paths it gave Skia and what Skia returned.
//!
//! The format is described in tests/reference/recorder.cpp. A path is kept as its raw arrays,
//! with float bit patterns, so that a case is rebuilt and compared exactly.

use std::fmt::Write;

use valo_pathops::{SkPath, SkPathFillType, SkPathOp, SkPathVerb, SkPoint};

/// `Case` is one call of Skia's `Op` or `Simplify` and Skia's result.
pub struct Case {
    pub name: String,
    pub kind: Kind,
    /// The Skia test helper that made the call, or `direct`.
    pub helper: String,
    /// What the helper expects: `yes`, `no`, `flaky` or `unchecked`.
    pub expect: String,
    /// False when Skia's own test run skips this test.
    pub skia_runs: bool,
    pub inputs: Vec<RawPath>,
    /// `None` when Skia returned no path.
    pub result: Option<RawPath>,
}

/// `Kind` is which operation a case calls.
#[derive(Clone, Copy, Debug)]
pub enum Kind {
    Op(SkPathOp),
    Simplify,
}

/// `RawPath` is a path as Skia stores it.
#[derive(Clone, Debug, PartialEq)]
pub struct RawPath {
    pub fill_type: SkPathFillType,
    /// `None` for Skia's error path, which holds nothing: the path made from non-finite input.
    pub arrays: Option<PathArrays>,
}

/// `PathArrays` are a path's verbs, points and conic weights, each float as its bit pattern.
#[derive(Clone, Debug, PartialEq)]
pub struct PathArrays {
    pub verbs: Vec<SkPathVerb>,
    pub points: Vec<(u32, u32)>,
    pub weights: Vec<u32>,
}

impl RawPath {
    /// `of` gives the arrays `path` stores.
    pub fn of(path: &SkPath) -> RawPath {
        RawPath {
            fill_type: path.fill_type(),
            arrays: Some(PathArrays {
                verbs: path.verbs().to_vec(),
                points: path
                    .points()
                    .iter()
                    .map(|point| (point.x.to_bits(), point.y.to_bits()))
                    .collect(),
                weights: path
                    .conic_weights()
                    .iter()
                    .map(|weight| weight.to_bits())
                    .collect(),
            }),
        }
    }

    /// `build` makes the path with `SkPath::from_raw`, as the reference rebuilt it with
    /// `SkPath::Raw`: the error path from a NaN point, which both reject.
    pub fn build(&self) -> SkPath {
        match &self.arrays {
            Some(arrays) => SkPath::from_raw(
                arrays.verbs.clone(),
                arrays
                    .points
                    .iter()
                    .map(|&(x, y)| SkPoint {
                        x: f32::from_bits(x),
                        y: f32::from_bits(y),
                    })
                    .collect(),
                arrays
                    .weights
                    .iter()
                    .map(|&weight| f32::from_bits(weight))
                    .collect(),
                self.fill_type,
            ),
            None => SkPath::from_raw(
                vec![SkPathVerb::Move],
                vec![SkPoint {
                    x: f32::NAN,
                    y: f32::NAN,
                }],
                Vec::new(),
                self.fill_type,
            ),
        }
    }

    /// `matches` says whether `path` is this path bit for bit. The error path is told only by
    /// being empty, since `SkPath` does not say whether it is finite.
    pub fn matches(&self, path: &SkPath) -> bool {
        let found = RawPath::of(path);
        match &self.arrays {
            Some(_) => *self == found,
            None => {
                let empty = PathArrays {
                    verbs: Vec::new(),
                    points: Vec::new(),
                    weights: Vec::new(),
                };
                found.fill_type == self.fill_type && found.arrays == Some(empty)
            }
        }
    }

    /// `describe` lists the path one verb per line, each float as a value and its bits.
    pub fn describe(&self) -> String {
        let mut text = format!("{:?}", self.fill_type);
        let Some(arrays) = &self.arrays else {
            return text + " (not finite)\n";
        };
        text.push('\n');
        let mut points = arrays.points.iter();
        let mut weights = arrays.weights.iter();
        for verb in &arrays.verbs {
            let _ = write!(text, "  {verb:?}");
            for _ in 0..points_added_by(*verb) {
                match points.next() {
                    Some(&(x, y)) => {
                        let _ = write!(
                            text,
                            " ({}, {}) [{x:08x} {y:08x}]",
                            f32::from_bits(x),
                            f32::from_bits(y)
                        );
                    }
                    None => text.push_str(" (missing point)"),
                }
            }
            if *verb == SkPathVerb::Conic {
                match weights.next() {
                    Some(&weight) => {
                        let _ = write!(text, " weight {} [{weight:08x}]", f32::from_bits(weight));
                    }
                    None => text.push_str(" (missing weight)"),
                }
            }
            text.push('\n');
        }
        for &(x, y) in points {
            let _ = writeln!(
                text,
                "  extra point ({}, {}) [{x:08x} {y:08x}]",
                f32::from_bits(x),
                f32::from_bits(y)
            );
        }
        for &weight in weights {
            let _ = writeln!(
                text,
                "  extra weight {} [{weight:08x}]",
                f32::from_bits(weight)
            );
        }
        text
    }
}

fn points_added_by(verb: SkPathVerb) -> usize {
    match verb {
        SkPathVerb::Move | SkPathVerb::Line => 1,
        SkPathVerb::Quad | SkPathVerb::Conic => 2,
        SkPathVerb::Cubic => 3,
        SkPathVerb::Close => 0,
    }
}

/// `parse` reads a fixture file's cases.
pub fn parse(text: &str) -> Result<Vec<Case>, String> {
    let mut cases: Vec<Case> = Vec::new();
    for (index, line) in text.lines().enumerate() {
        parse_line(line, &mut cases).map_err(|error| format!("line {}: {error}", index + 1))?;
    }
    Ok(cases)
}

fn parse_line(line: &str, cases: &mut Vec<Case>) -> Result<(), String> {
    if line.is_empty() || line.starts_with('#') {
        return Ok(());
    }
    let (key, value) = line.split_once(' ').ok_or("a line without a value")?;
    if key == "case" {
        cases.push(Case {
            name: value.to_string(),
            kind: Kind::Simplify,
            helper: String::new(),
            expect: String::new(),
            skia_runs: true,
            inputs: Vec::new(),
            result: None,
        });
        return Ok(());
    }
    let case = cases.last_mut().ok_or("a line before the first case")?;
    match key {
        "kind" => case.kind = parse_kind(value)?,
        "helper" => case.helper = value.to_string(),
        "expect" => case.expect = value.to_string(),
        "skia-runs" => case.skia_runs = value == "yes",
        "input" => case.inputs.push(parse_path(value)?),
        "result" if value == "none" => case.result = None,
        "result" => case.result = Some(parse_path(value)?),
        _ => return Err(format!("unknown key {key}")),
    }
    Ok(())
}

fn parse_kind(value: &str) -> Result<Kind, String> {
    let op = match value {
        "simplify" => return Ok(Kind::Simplify),
        "op difference" => SkPathOp::Difference,
        "op intersect" => SkPathOp::Intersect,
        "op union" => SkPathOp::Union,
        "op xor" => SkPathOp::Xor,
        "op reverse-difference" => SkPathOp::ReverseDifference,
        _ => return Err(format!("unknown kind {value}")),
    };
    Ok(Kind::Op(op))
}

fn parse_path(value: &str) -> Result<RawPath, String> {
    let fields: Vec<&str> = value.split(' ').collect();
    let fill_type = parse_fill_type(fields[0])?;
    match fields[1..] {
        ["nonfinite"] => Ok(RawPath {
            fill_type,
            arrays: None,
        }),
        [verbs, points, weights] => Ok(RawPath {
            fill_type,
            arrays: Some(PathArrays {
                verbs: parse_verbs(verbs)?,
                points: parse_points(points)?,
                weights: parse_list(weights, parse_bits)?,
            }),
        }),
        _ => Err(format!("a path of {} fields", fields.len())),
    }
}

fn parse_fill_type(text: &str) -> Result<SkPathFillType, String> {
    match text {
        "winding" => Ok(SkPathFillType::Winding),
        "evenodd" => Ok(SkPathFillType::EvenOdd),
        "inverse-winding" => Ok(SkPathFillType::InverseWinding),
        "inverse-evenodd" => Ok(SkPathFillType::InverseEvenOdd),
        _ => Err(format!("unknown fill type {text}")),
    }
}

fn parse_verbs(text: &str) -> Result<Vec<SkPathVerb>, String> {
    if text == "-" {
        return Ok(Vec::new());
    }
    text.chars()
        .map(|letter| match letter {
            'M' => Ok(SkPathVerb::Move),
            'L' => Ok(SkPathVerb::Line),
            'Q' => Ok(SkPathVerb::Quad),
            'K' => Ok(SkPathVerb::Conic),
            'C' => Ok(SkPathVerb::Cubic),
            'Z' => Ok(SkPathVerb::Close),
            _ => Err(format!("unknown verb {letter}")),
        })
        .collect()
}

fn parse_points(text: &str) -> Result<Vec<(u32, u32)>, String> {
    parse_list(text, |point| {
        let (x, y) = point
            .split_once(':')
            .ok_or_else(|| format!("a point without a colon: {point}"))?;
        Ok((parse_bits(x)?, parse_bits(y)?))
    })
}

fn parse_list<T>(
    text: &str,
    parse_item: impl Fn(&str) -> Result<T, String>,
) -> Result<Vec<T>, String> {
    if text == "-" {
        return Ok(Vec::new());
    }
    text.split(',').map(parse_item).collect()
}

fn parse_bits(text: &str) -> Result<u32, String> {
    u32::from_str_radix(text, 16).map_err(|error| format!("bad float bits {text}: {error}"))
}
