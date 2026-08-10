//! Signed distance fields from coverage bitmaps — pure CPU image processing.
//! The mapbox TinySDF shape, chosen by benchmark:
//! the 1× ANTI-ALIASED coverage seeds two squared-distance grids (partial
//! alpha = sub-pixel edge offset, so no supersampling is needed), each swept
//! by the Felzenszwalb–Huttenlocher exact separable EDT. mapbox-gl runs this
//! exact algorithm at runtime for CJK label glyphs — the same workload.
//! The GPU half (atlas residency + the smoothstep shader) lives in
//! valo-renderer.

const INF: f32 = 1e20;

/// Signed distance (px, positive INSIDE) per pixel of an AA coverage bitmap.
pub(crate) fn signed_distances(coverage: &[u8], w: usize, h: usize) -> Vec<f32> {
    debug_assert_eq!(coverage.len(), w * h);
    let (mut outer, mut inner) = seed_from_alpha(coverage);
    edt_2d(&mut outer, w, h);
    edt_2d(&mut inner, w, h);
    (0..w * h)
        .map(|i| inner[i].sqrt() - outer[i].sqrt())
        .collect()
}

/// TinySDF seeding: solid pixels are 0/∞ sites; partial pixels seed the
/// squared FRACTIONAL offset of the 0.5-coverage edge — the sub-pixel
/// information the old pipeline recovered by rasterizing at 2×.
fn seed_from_alpha(coverage: &[u8]) -> (Vec<f32>, Vec<f32>) {
    let mut outer = vec![INF; coverage.len()];
    let mut inner = vec![0.0f32; coverage.len()];
    for (i, &c) in coverage.iter().enumerate() {
        match c {
            0 => {}
            255 => {
                outer[i] = 0.0;
                inner[i] = INF;
            }
            _ => {
                let d = 0.5 - c as f32 / 255.0;
                outer[i] = if d > 0.0 { d * d } else { 0.0 };
                inner[i] = if d < 0.0 { d * d } else { 0.0 };
            }
        }
    }
    (outer, inner)
}

/// Exact squared EDT, separable: one 1D pass per column, then per row
/// (Felzenszwalb & Huttenlocher 2012).
fn edt_2d(grid: &mut [f32], w: usize, h: usize) {
    let mut scratch = Scratch::new(w.max(h));
    let mut column = vec![0.0f32; h];
    for x in 0..w {
        for y in 0..h {
            column[y] = grid[y * w + x];
        }
        scratch.edt_1d(&column, h);
        for y in 0..h {
            grid[y * w + x] = scratch.d[y];
        }
    }
    let mut row = vec![0.0f32; w];
    for y in 0..h {
        row.copy_from_slice(&grid[y * w..(y + 1) * w]);
        scratch.edt_1d(&row, w);
        grid[y * w..(y + 1) * w].copy_from_slice(&scratch.d[..w]);
    }
}

/// The 1D pass's reusable arrays: parabola apexes `v`, boundaries `z`,
/// output `d`.
struct Scratch {
    d: Vec<f32>,
    v: Vec<usize>,
    z: Vec<f32>,
}

impl Scratch {
    fn new(n: usize) -> Self {
        Scratch {
            d: vec![0.0; n],
            v: vec![0; n],
            z: vec![0.0; n + 1],
        }
    }

    /// Lower envelope of the parabolas y = (x − q)² + f[q], then read the
    /// envelope back out — exact squared distances in one linear pass.
    fn edt_1d(&mut self, f: &[f32], n: usize) {
        let (v, z, d) = (&mut self.v, &mut self.z, &mut self.d);
        let mut k = 0;
        v[0] = 0;
        z[0] = -INF;
        z[1] = INF;
        for q in 1..n {
            let mut s = intersect(f, q, v[k]);
            while s <= z[k] {
                k -= 1;
                s = intersect(f, q, v[k]);
            }
            k += 1;
            v[k] = q;
            z[k] = s;
            z[k + 1] = INF;
        }
        k = 0;
        for (q, out) in d.iter_mut().enumerate().take(n) {
            while z[k + 1] < q as f32 {
                k += 1;
            }
            let dq = q as f32 - v[k] as f32;
            *out = dq * dq + f[v[k]];
        }
    }
}

/// Where parabolas rooted at `q` and `p` cross (x of equal height).
fn intersect(f: &[f32], q: usize, p: usize) -> f32 {
    let (fq, fp) = (f[q] + (q * q) as f32, f[p] + (p * p) as f32);
    (fq - fp) / (2 * q - 2 * p) as f32
}

/// Encode signed px distances into A8: 128 = edge, ±`spread` px span the full range. The
/// shader recovers coverage with a screen-space smoothstep around 0.5.
pub(crate) fn encode(field: &[f32], spread_px: f32) -> Vec<u8> {
    field
        .iter()
        .map(|d| ((0.5 + d / (2.0 * spread_px)).clamp(0.0, 1.0) * 255.0).round() as u8)
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A filled circle's SDF must be ≈ (radius − distance-from-center) everywhere near it.
    #[test]
    fn circle_distances_match_geometry() {
        let (w, h, r) = (64usize, 64usize, 20.0f32);
        let cov: Vec<u8> = (0..w * h)
            .map(|i| {
                let (x, y) = ((i % w) as f32 - 32.0, (i / w) as f32 - 32.0);
                if (x * x + y * y).sqrt() <= r {
                    255
                } else {
                    0
                }
            })
            .collect();
        let field = signed_distances(&cov, w, h);
        for (i, d) in field.iter().enumerate() {
            let (x, y) = ((i % w) as f32 - 32.0, (i / w) as f32 - 32.0);
            let expected = r - (x * x + y * y).sqrt();
            // The EDT is exact; the binary edge itself contributes ±0.5px.
            assert!(
                (d - expected).abs() <= 1.0,
                "at ({x},{y}): sdf {d} vs geometric {expected}"
            );
        }
    }

    /// AA coverage shifts the recovered edge sub-pixel — the property the
    /// old pipeline bought with a 2× supersample.
    #[test]
    fn partial_coverage_moves_the_edge_subpixel() {
        // A vertical edge whose boundary column is 25% covered vs 75%.
        let (w, h) = (16usize, 8usize);
        let strip = |edge_alpha: u8| -> Vec<f32> {
            let cov: Vec<u8> = (0..w * h)
                .map(|i| match (i % w).cmp(&8) {
                    std::cmp::Ordering::Less => 255,
                    std::cmp::Ordering::Equal => edge_alpha,
                    std::cmp::Ordering::Greater => 0,
                })
                .collect();
            signed_distances(&cov, w, h)
        };
        let quarter = strip(64);
        let three_quarters = strip(191);
        // More coverage on the boundary column ⇒ the zero crossing sits
        // further OUT ⇒ every nearby sample reads "more inside".
        let at = |f: &[f32], x: usize| f[4 * w + x];
        assert!(at(&three_quarters, 8) > at(&quarter, 8));
        assert!(at(&three_quarters, 7) >= at(&quarter, 7));
    }
}
