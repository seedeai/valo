// One uniform record serves every fragment family (512 B = the dynamic-offset
// stride anyway): mvp + color + a generic payload the family interprets.
// Layout contract (mirrored in renderer.rs `payload` constants):
//   payload[0] = local-space rect (x, y, w, h) — vs_quad derives the LOCAL
//                varying from it, so gradients/images live in draw space
//   payload[1] = family geometry: image uv mapping / gradient points
//   payload[2] = (stop_count, angle, spread_mode, radial fy)
//   payload[3..5) = 8 gradient stop offsets
//   payload[5..13) = 8 gradient stop colors (PREMULTIPLIED)
//   payload[13..15) = inverse gradient/pattern local matrix (a,b,c,d | tx,ty,_,_)
//   payload[15..17) = two-point conical setup + its flags
//   payload[17..22) = colour matrix rows + translation column; slot 17 alone
//                     carries the blend filter's premultiplied source colour
// Colors are premultiplied everywhere; depth (the draw's slot) rides in mvp.
// plan.rs's `PAYLOAD_*` constants are the authority for all of this.

struct DrawUniforms {
    mvp: mat4x4<f32>,
    color: vec4<f32>,
    payload: array<vec4<f32>, 27>,
};

@group(0) @binding(0) var<uniform> u: DrawUniforms;

struct VsOut {
    @builtin(position) pos: vec4<f32>,
    @location(0) local: vec2<f32>,
};

@vertex
fn vs_quad(@builtin(vertex_index) vi: u32) -> VsOut {
    // Two CCW triangles over the unit square.
    var corners = array<vec2<f32>, 6>(
        vec2(0.0, 0.0), vec2(1.0, 0.0), vec2(0.0, 1.0),
        vec2(1.0, 0.0), vec2(1.0, 1.0), vec2(0.0, 1.0),
    );
    let corner = corners[vi];
    let rect = u.payload[0];
    var out: VsOut;
    out.pos = u.mvp * vec4<f32>(corner, 0.0, 1.0);
    out.local = rect.xy + corner * rect.zw;
    return out;
}

@vertex
fn vs_mesh(@location(0) p: vec2<f32>) -> VsOut {
    var out: VsOut;
    out.pos = u.mvp * vec4<f32>(p, 0.0, 1.0);
    out.local = p;
    return out;
}

@fragment
fn fs_solid(in: VsOut) -> @location(0) vec4<f32> {
    return u.color;
}

// ── image ───────────────────────────────────────────────────────────────────

@group(1) @binding(0) var t_tex: texture_2d<f32>;
@group(1) @binding(1) var t_samp: sampler;

@fragment
fn fs_image(in: VsOut) -> @location(0) vec4<f32> {
    // uv = local × scale + offset (src→dst mapping precomputed CPU-side);
    // tiling comes from the sampler's address modes on out-of-range uv.
    let m = u.payload[1];
    let uv = in.local * m.xy + m.zw;
    return textureSample(t_tex, t_samp, uv) * u.color;
}

// ── gradients (uniform stops, ≤8) ───────────────────────────────────────────

fn stop_offset(i: u32) -> f32 {
    let v = u.payload[3u + (i >> 2u)];
    let lane = i & 3u;
    if lane == 0u { return v.x; }
    if lane == 1u { return v.y; }
    if lane == 2u { return v.z; }
    return v.w;
}

/// Piecewise-linear ramp over premultiplied stop colors.
fn ramp(t: f32) -> vec4<f32> {
    let count = u32(u.payload[2].x);
    var prev_off = stop_offset(0u);
    var prev_col = u.payload[5u];
    if t <= prev_off {
        return prev_col;
    }
    for (var i = 1u; i < count; i = i + 1u) {
        let off = stop_offset(i);
        let col = u.payload[5u + i];
        if t <= off {
            let span = max(off - prev_off, 1e-6);
            return mix(prev_col, col, (t - prev_off) / span);
        }
        prev_off = off;
        prev_col = col;
    }
    return prev_col;
}

/// Gradients evaluate in their OWN space (Skia's local matrix):
/// payload[13..15) carry the inverse mapping draw-local → gradient
/// coords. Identity for plain gradients — this is a no-op then.
fn gradient_point(p: vec2<f32>) -> vec2<f32> {
    let m = u.payload[13];
    let t = u.payload[14];
    return vec2(m.x * p.x + m.z * p.y + t.x, m.y * p.x + m.w * p.y + t.y);
}

/// What lives outside 0..1 (payload[2].z): 0 pad (clamp), 1 repeat
/// (tile), 2 reflect (mirror every other tile). fract() handles negative
/// t for both periodic modes.
fn spread(t: f32) -> f32 {
    let mode = u32(u.payload[2].z);
    if mode == 1u {
        return fract(t);
    }
    if mode == 2u {
        let f = fract(t * 0.5) * 2.0;
        return 1.0 - abs(f - 1.0);
    }
    return clamp(t, 0.0, 1.0);
}

fn linear_t(local: vec2<f32>) -> f32 {
    let g = u.payload[1]; // (ax, ay, bx, by), gradient space
    let p = gradient_point(local);
    let d = g.zw - g.xy;
    return spread(dot(p - g.xy, d) / max(dot(d, d), 1e-6));
}

@fragment
fn fs_linear(in: VsOut) -> @location(0) vec4<f32> {
    return ramp(linear_t(in.local)) * u.color;
}

/// Two-point conical `t`, plus a validity flag: some points of a general
/// conical gradient are covered by NEITHER circle and must stay
/// transparent. `payload[15] = (kind, r1_in_unit_space, focal, sign)` and
/// `payload[16] = (swapped, focal_on_circle, well_behaved, _)`, both settled
/// on the CPU — the position arriving here is already in focal space when
/// the general case needs it.
fn radial_t(local: vec2<f32>) -> vec2<f32> {
    let setup = u.payload[15];
    let kind = setup.x;
    let p = gradient_point(local);

    // Concentric: t is just the fraction of the way between the two radii.
    if kind == 0.0 {
        let g = u.payload[1];
        let start_radius = setup.y;
        let end_radius = setup.z;
        let distance = length(p - g.xy);
        return vec2(spread((distance - start_radius) / (end_radius - start_radius)), 1.0);
    }
    // Identical circles paint nothing at all.
    if kind == 2.0 {
        return vec2(0.0, 0.0);
    }
    // Equal radii: the gradient sweeps the strip between the circles' common
    // tangents, and everything beyond those tangents is uncovered.
    if kind == 3.0 {
        let radius_squared = setup.y;
        let half_span = radius_squared - p.y * p.y;
        if half_span < 0.0 {
            return vec2(0.0, 0.0);
        }
        return vec2(spread(p.x + sqrt(half_span)), 1.0);
    }

    // The general case, continuing Skia's algorithm from step 5.
    let flags = u.payload[16];
    let is_swapped = flags.x > 0.5;
    let is_focal_on_circle = flags.y > 0.5;
    let is_well_behaved = flags.z > 0.5;
    let radius_in_unit_space = setup.y;
    let focal = setup.z;
    let radius_sign = setup.w;

    var x_t = -1.0;
    if is_focal_on_circle {
        x_t = dot(p, p) / p.x;
    } else if is_well_behaved {
        x_t = length(p) - p.x / radius_in_unit_space;
    } else {
        let discriminant = p.x * p.x - p.y * p.y;
        if discriminant >= 0.0 {
            let root = sqrt(discriminant);
            if is_swapped || radius_sign < 0.0 {
                x_t = -root - p.x / radius_in_unit_space;
            } else {
                x_t = root - p.x / radius_in_unit_space;
            }
        }
    }
    // Behind the focal cone: outside the gradient entirely.
    if !is_well_behaved && x_t < 0.0 {
        return vec2(0.0, 0.0);
    }

    var t = focal + radius_sign * x_t;
    if is_swapped {
        t = 1.0 - t;
    }
    return vec2(spread(t), 1.0);
}

@fragment
fn fs_radial(in: VsOut) -> @location(0) vec4<f32> {
    let solved = radial_t(in.local);
    return ramp(solved.x) * u.color * solved.y;
}

const TAU: f32 = 6.28318530718;

fn sweep_t(local: vec2<f32>) -> f32 {
    let g = u.payload[1]; // (cx, cy, _, _); start angle in payload[2].y
    let v = gradient_point(local) - g.xy;
    return fract((atan2(v.y, v.x) - u.payload[2].y) / TAU);
}

@fragment
fn fs_sweep(in: VsOut) -> @location(0) vec4<f32> {
    return ramp(sweep_t(in.local)) * u.color;
}

// ── ramp gradients (>8 stops): Impeller's texture path ──────────────────────
// The stop list lives in a baked N×1 premultiplied texture; payload[2].x
// carries N so t maps to texel CENTERS (linear filtering interpolates
// between them exactly like the analytic ramp).

fn sample_ramp(t: f32) -> vec4<f32> {
    let n = u.payload[2].x;
    let uv = vec2((t * (n - 1.0) + 0.5) / n, 0.5);
    return textureSample(t_tex, t_samp, uv);
}

@fragment
fn fs_linear_ramp(in: VsOut) -> @location(0) vec4<f32> {
    return sample_ramp(linear_t(in.local)) * u.color;
}

@fragment
fn fs_radial_ramp(in: VsOut) -> @location(0) vec4<f32> {
    let solved = radial_t(in.local);
    return sample_ramp(solved.x) * u.color * solved.y;
}

@fragment
fn fs_sweep_ramp(in: VsOut) -> @location(0) vec4<f32> {
    return sample_ramp(sweep_t(in.local)) * u.color;
}

// ── mask composite ──────────────────────────────────────────────────────────
// The mask layer's texture as COVERAGE, drawn with DstIn over the whole
// enclosing layer. payload[1] maps local → mask uv; payload[2].x picks
// luminance (1) or alpha (0). Outside the mask texture coverage is 0 —
// that erasure of unmasked content is the point (never clamp-smear edge
// texels outward).

@fragment
fn fs_mask_composite(in: VsOut) -> @location(0) vec4<f32> {
    let m = u.payload[1];
    let uv = in.local * m.xy + m.zw;
    let s = textureSample(t_tex, t_samp, clamp(uv, vec2(0.0), vec2(1.0)));
    let inside = f32(all(uv >= vec2(0.0)) && all(uv <= vec2(1.0)));
    var coverage = s.a;
    if u.payload[2].x > 0.5 {
        // Premultiplied luma = luma(straight) × alpha in one dot (BT.709).
        coverage = dot(s.rgb, vec3(0.2126, 0.7152, 0.0722));
    }
    return vec4(0.0, 0.0, 0.0, coverage * inside * u.color.a);
}

// ── advanced (dst-reading) blends ───────────────────────────────────────────
// The pass broke before these draws: `t_dst` holds a snapshot of the target,
// sampled at framebuffer coords (uv = position / payload[2].zw). The shader
// computes blend + composite in one (PDF/W3C compositing formulas over
// UNpremultiplied color); pipeline blending is OFF — output replaces dst.
// Mode ids in payload[2].x match `advanced_mode_id` in pipelines.rs.

fn unpremul(c: vec4<f32>) -> vec3<f32> {
    // max() instead of select(): select evaluates BOTH branches, and /0 on
    // some backends poisons the result even when discarded.
    return c.rgb / max(c.a, 1e-6);
}

fn lum(c: vec3<f32>) -> f32 {
    return dot(c, vec3(0.3, 0.59, 0.11));
}

fn clip_color(c_in: vec3<f32>) -> vec3<f32> {
    var c = c_in;
    let l = lum(c);
    let n = min(min(c.r, c.g), c.b);
    let x = max(max(c.r, c.g), c.b);
    if n < 0.0 {
        c = l + (c - l) * l / (l - n);
    }
    if x > 1.0 {
        c = l + (c - l) * (1.0 - l) / (x - l);
    }
    return c;
}

fn set_lum(c: vec3<f32>, l: f32) -> vec3<f32> {
    return clip_color(c + (l - lum(c)));
}

fn sat(c: vec3<f32>) -> f32 {
    return max(max(c.r, c.g), c.b) - min(min(c.r, c.g), c.b);
}

fn set_sat(c: vec3<f32>, s: f32) -> vec3<f32> {
    let cmin = min(min(c.r, c.g), c.b);
    let cmax = max(max(c.r, c.g), c.b);
    if cmax > cmin {
        return (c - cmin) * s / (cmax - cmin);
    }
    return vec3(0.0);
}

fn hard_light(s: vec3<f32>, d: vec3<f32>) -> vec3<f32> {
    return select(1.0 - 2.0 * (1.0 - s) * (1.0 - d), 2.0 * s * d, s <= vec3(0.5));
}

fn soft_light(s: vec3<f32>, d: vec3<f32>) -> vec3<f32> {
    let dd = select(sqrt(d), ((16.0 * d - 12.0) * d + 4.0) * d, d <= vec3(0.25));
    return select(d + (2.0 * s - 1.0) * (dd - d), d - (1.0 - 2.0 * s) * d * (1.0 - d), s <= vec3(0.5));
}

fn color_dodge(s: vec3<f32>, d: vec3<f32>) -> vec3<f32> {
    let r = min(vec3(1.0), d / max(1.0 - s, vec3(1e-6)));
    return select(select(r, vec3(1.0), s >= vec3(1.0)), vec3(0.0), d <= vec3(0.0));
}

fn color_burn(s: vec3<f32>, d: vec3<f32>) -> vec3<f32> {
    let r = 1.0 - min(vec3(1.0), (1.0 - d) / max(s, vec3(1e-6)));
    return select(select(r, vec3(0.0), s <= vec3(0.0)), vec3(1.0), d >= vec3(1.0));
}

fn blend_advanced(mode: u32, s: vec3<f32>, d: vec3<f32>) -> vec3<f32> {
    switch mode {
        case 0u: { return s * d; }                       // Multiply
        case 1u: { return hard_light(d, s); }            // Overlay
        case 2u: { return min(s, d); }                   // Darken
        case 3u: { return max(s, d); }                   // Lighten
        case 4u: { return color_dodge(s, d); }           // ColorDodge
        case 5u: { return color_burn(s, d); }            // ColorBurn
        case 6u: { return hard_light(s, d); }            // HardLight
        case 7u: { return soft_light(s, d); }            // SoftLight
        case 8u: { return abs(s - d); }                  // Difference
        case 9u: { return s + d - 2.0 * s * d; }         // Exclusion
        case 10u: { return set_lum(set_sat(s, sat(d)), lum(d)); } // Hue
        case 11u: { return set_lum(set_sat(d, sat(s)), lum(d)); } // Saturation
        case 12u: { return set_lum(s, lum(d)); }         // Color
        default: { return set_lum(d, lum(s)); }          // Luminosity
    }
}

/// blend + composite (PDF §7.2.: co = αs(1−αd)·Cs + αd(1−αs)·Cd + αsαd·B).
fn composite_advanced(mode: u32, src: vec4<f32>, dst: vec4<f32>) -> vec4<f32> {
    let s = unpremul(src);
    let d = unpremul(dst);
    let b = blend_advanced(mode, s, d);
    let sa = src.a;
    let da = dst.a;
    let rgb = s * sa * (1.0 - da) + d * da * (1.0 - sa) + b * sa * da;
    return vec4(rgb, sa + da * (1.0 - sa));
}

fn dst_sample(pos: vec4<f32>) -> vec4<f32> {
    let uv = pos.xy / u.payload[2].zw; // target size in misc.zw
    return textureSample(t_tex, t_samp, uv);
}

/// Solid src × snapshot dst (group1 texture = the dst snapshot).
@fragment
fn fs_blend_solid(in: VsOut) -> @location(0) vec4<f32> {
    let mode = u32(u.payload[2].x);
    return composite_advanced(mode, u.color, dst_sample(in.pos));
}

// Texture src (a layer or desugared draw) × snapshot dst.
@group(1) @binding(2) var t_src: texture_2d<f32>;

@fragment
fn fs_blend_texture(in: VsOut) -> @location(0) vec4<f32> {
    let m = u.payload[1];
    let src_uv = in.local * m.xy + m.zw;
    let src = textureSample(t_src, t_samp, src_uv) * u.color;
    let mode = u32(u.payload[2].x);
    return composite_advanced(mode, src, dst_sample(in.pos));
}

// ── patterns ────────────────────────────────────────────────────────────────
// An image tiled across the shape. `gradient_point` already carries the local
// position through the paint's inverse local matrix, so all that remains is
// pattern pixels → uv. Tiling and filtering ride the sampler's address modes,
// exactly as an image DRAW's do, so a repeat costs nothing in the shader.

@fragment
fn fs_pattern(in: VsOut) -> @location(0) vec4<f32> {
    let uv = gradient_point(in.local) * u.payload[1].xy;
    return textureSample(t_tex, t_samp, uv) * u.color;
}

// ── colour filters (filter passes only) ─────────────────────────────────────
// Run over a layer's texture BEFORE any blur, so the blur spreads filtered
// pixels. payload[1] maps this pass's local space to source uv, exactly as the
// blur passes do. Impeller runs colour matrices as a pass over a snapshot too
// (ColorMatrixFilterContents) rather than folding them into every draw shader.

/// payload[17..21] = the 4×5's rows, payload[21] = its translation column.
@fragment
fn fs_color_matrix(in: VsOut) -> @location(0) vec4<f32> {
    let m = u.payload[1];
    let texel = textureSample(t_tex, t_samp, in.local * m.xy + m.zw);
    // Colour matrices are defined on STRAIGHT colour; layers are premultiplied.
    let color = vec4(unpremul(texel), texel.a);
    let filtered = clamp(
        vec4(
            dot(u.payload[17], color),
            dot(u.payload[18], color),
            dot(u.payload[19], color),
            dot(u.payload[20], color),
        ) + u.payload[21],
        vec4(0.0),
        vec4(1.0),
    );
    return vec4(filtered.rgb * filtered.a, filtered.a);
}

/// Porter-Duff over PREMULTIPLIED colour: result = src·fs + dst·fd, with the
/// three modes that aren't a plain factor pair returning directly.
fn composite_porter_duff(mode: u32, src: vec4<f32>, dst: vec4<f32>) -> vec4<f32> {
    var fs = 0.0;
    var fd = 0.0;
    switch mode {
        case 0u: {}                                        // Clear
        case 1u: { fs = 1.0; }                             // Src
        case 2u: { fd = 1.0; }                             // Dst
        case 3u: { fs = 1.0; fd = 1.0 - src.a; }           // SrcOver
        case 4u: { fs = 1.0 - dst.a; fd = 1.0; }           // DstOver
        case 5u: { fs = dst.a; }                           // SrcIn
        case 6u: { fd = src.a; }                           // DstIn
        case 7u: { fs = 1.0 - dst.a; }                     // SrcOut
        case 8u: { fd = 1.0 - src.a; }                     // DstOut
        case 9u: { fs = dst.a; fd = 1.0 - src.a; }         // SrcAtop
        case 10u: { fs = 1.0 - dst.a; fd = src.a; }        // DstAtop
        case 11u: { fs = 1.0 - dst.a; fd = 1.0 - src.a; }  // Xor
        case 12u: { return min(src + dst, vec4(1.0)); }    // Plus (clamped)
        case 13u: { return src * dst; }                    // Modulate
        default: { return src + dst - src * dst; }         // Screen
    }
    return src * fs + dst * fd;
}

/// A constant colour blended AS THE SOURCE over the layer — Flutter's
/// `ColorFilter.mode`. payload[17] = that colour premultiplied; payload[2].x =
/// the mode, pipeline-blendable ids first, advanced ones offset by 15.
@fragment
fn fs_color_blend(in: VsOut) -> @location(0) vec4<f32> {
    let m = u.payload[1];
    let dst = textureSample(t_tex, t_samp, in.local * m.xy + m.zw);
    let src = u.payload[17];
    let mode = u32(u.payload[2].x);
    if mode >= 15u {
        return composite_advanced(mode - 15u, src, dst);
    }
    return composite_porter_duff(mode, src, dst);
}

// ── gaussian blur, one direction ────────────────────────────────────────────
// Separable: H then V turns O(r²) taps per pixel into O(2r). Runs in filter
// passes (1-sample, no depth). payload[2] = (sigma, radius, step.x, step.y)
// with step = one texel along the blur direction in uv units; radius 0 makes
// this a plain bilinear resample (the downsample pass). Premultiplied color
// averages linearly, so no unpremul dance is needed.

@fragment
fn fs_blur(in: VsOut) -> @location(0) vec4<f32> {
    let m = u.payload[1];
    let uv = in.local * m.xy + m.zw;
    let sigma = max(u.payload[2].x, 0.1);
    let radius = i32(u.payload[2].y);
    let step = u.payload[2].zw;
    var total = textureSample(t_tex, t_samp, uv);
    var total_weight = 1.0;
    for (var i = 1; i <= radius; i = i + 1) {
        let w = exp(-f32(i * i) / (2.0 * sigma * sigma));
        let offset = step * f32(i);
        total += (textureSample(t_tex, t_samp, uv + offset) +
                  textureSample(t_tex, t_samp, uv - offset)) * w;
        total_weight += 2.0 * w;
    }
    return total / total_weight;
}

// ── closed-form blurred rounded rect (Impeller rrect_blur) ──────────────────
// Why a box shadow is ONE draw: a 2-D gaussian ∗ box separates into 1-D
// convolutions, and a blurred step edge is an erf. Exact along x, 4-sample
// gauss-weighted integration along y where the rounded profile varies —
// Evan Wallace's "fast rounded rectangle shadows" formulation, generalized
// to PER-CORNER radii (each row's left/right bound uses its own corner).
// payload[1] = rrect (x0, y0, x1, y1) local; payload[2] = (sigma, style, _, _);
// payload[3] = radii (tl, tr, br, bl). Style ids match `blur_style_id`.

fn gauss(x: f32, sigma: f32) -> f32 {
    return exp(-(x * x) / (2.0 * sigma * sigma)) / (2.5066282746 * sigma);
}

fn erf2(x: vec2<f32>) -> vec2<f32> {
    let s = sign(x);
    let a = abs(x);
    var y = 1.0 + (0.278393 + (0.230389 + 0.078108 * (a * a)) * a) * a;
    y = y * y;
    return s - s / (y * y);
}

/// How far a side edge is inset at row `y` (centered coords): zero on the
/// straight run, up to the corner radius across that corner's arc.
fn edge_inset(y: f32, half_y: f32, r_top: f32, r_bottom: f32) -> f32 {
    let r = select(r_bottom, r_top, y < 0.0);
    let d = min(half_y - r - abs(y), 0.0);
    return r - sqrt(max(0.0, r * r - d * d));
}

/// Blurred coverage along x at row offset `y`: gaussian integral over
/// [left(y), right(y)], each bound shaped by its own corners.
fn rrect_blur_x(x: f32, y: f32, sigma: f32, radii: vec4<f32>, half_size: vec2<f32>) -> f32 {
    let left = -half_size.x + edge_inset(y, half_size.y, radii.x, radii.w);
    let right = half_size.x - edge_inset(y, half_size.y, radii.y, radii.z);
    let integral = 0.5 + 0.5 * erf2(vec2(x - left, x - right) * (0.7071067812 / sigma));
    return integral.x - integral.y;
}

fn rrect_blur_coverage(p: vec2<f32>, half_size: vec2<f32>, sigma: f32, radii: vec4<f32>) -> f32 {
    // Integrate y only where the signal is non-zero: box rows within ±3σ.
    let start = clamp(-3.0 * sigma, p.y - half_size.y, p.y + half_size.y);
    let end = clamp(3.0 * sigma, p.y - half_size.y, p.y + half_size.y);
    let step = (end - start) / 4.0;
    var y = start + step * 0.5;
    var coverage = 0.0;
    for (var i = 0; i < 4; i = i + 1) {
        coverage += rrect_blur_x(p.x, p.y - y, sigma, radii, half_size) * gauss(y, sigma) * step;
        y += step;
    }
    return coverage;
}

/// Signed distance to the sharp rrect (per-corner) — the style mask.
fn rrect_sdf(p: vec2<f32>, half_size: vec2<f32>, radii: vec4<f32>) -> f32 {
    var r = radii.x; // tl
    if p.x >= 0.0 {
        r = select(radii.z, radii.y, p.y < 0.0); // br / tr
    } else if p.y >= 0.0 {
        r = radii.w; // bl
    }
    let q = abs(p) - half_size + vec2(r, r);
    return length(max(q, vec2(0.0))) + min(max(q.x, q.y), 0.0) - r;
}

/// Skia's blur styles from blurred coverage B and sharp coverage M:
/// Normal = B · Solid = M over B · Inner = B inside M · Outer = B outside M.
fn styled_coverage(style: u32, blurred: f32, sharp: f32) -> f32 {
    switch style {
        case 1u: { return sharp + blurred * (1.0 - sharp); }
        case 2u: { return blurred * sharp; }
        case 3u: { return blurred * (1.0 - sharp); }
        default: { return blurred; }
    }
}

@fragment
fn fs_rrect_blur(in: VsOut) -> @location(0) vec4<f32> {
    let r = u.payload[1];
    let sigma = max(u.payload[2].x, 0.05);
    let style = u32(u.payload[2].y);
    let radii = u.payload[3];
    let half_size = (r.zw - r.xy) * 0.5;
    let p = in.local - (r.xy + r.zw) * 0.5;
    let blurred = rrect_blur_coverage(p, half_size, sigma, radii);
    // Screen-space AA on the sharp edge (fwidth = local units per pixel).
    let d = rrect_sdf(p, half_size, radii);
    let sharp = clamp(0.5 - d / max(fwidth(d), 1e-4), 0.0, 1.0);
    return u.color * styled_coverage(style, blurred, sharp);
}

// ── blur style combine (general mask path) ──────────────────────────────────
// Runs as a filter pass after the blur chain: merges the blurred layer B
// (t_tex) with the SHARP layer M (t_src) so the composite stays one texture
// for any blend mode. payload[1] = B's uv mapping; payload[2] = (style, _,
// 1/w, 1/h) — M's uv is just local / layer size.

@fragment
fn fs_mask_combine(in: VsOut) -> @location(0) vec4<f32> {
    let mb = u.payload[1];
    let b = textureSample(t_tex, t_samp, in.local * mb.xy + mb.zw);
    let m = textureSample(t_src, t_samp, in.local * u.payload[2].zw);
    let style = u32(u.payload[2].x);
    switch style {
        case 1u: { return m + b * (1.0 - m.a); }
        case 2u: { return b * m.a; }
        default: { return b * (1.0 - m.a); }
    }
}

// ── text: atlas-masked glyph quads ──────────────────────────────────────────
// Vertices carry (pos, uv) into the R8 glyph atlas. Bitmap tier: coverage ×
// tint. SDF tier: threshold a distance field at 0.5 with screen-space AA —
// one raster serves every transform until the outline tier takes over.

struct TextOut {
    @builtin(position) pos: vec4<f32>,
    @location(0) uv: vec2<f32>,
};

@vertex
fn vs_text(@location(0) p: vec2<f32>, @location(1) uv: vec2<f32>) -> TextOut {
    var out: TextOut;
    out.pos = u.mvp * vec4<f32>(p, 0.0, 1.0);
    out.uv = uv;
    return out;
}

@fragment
fn fs_text(in: TextOut) -> @location(0) vec4<f32> {
    return u.color * textureSample(t_tex, t_samp, in.uv).r;
}

@fragment
fn fs_text_sdf(in: TextOut) -> @location(0) vec4<f32> {
    let d = textureSample(t_tex, t_samp, in.uv).r;
    let w = max(fwidth(d), 1e-4);
    return u.color * smoothstep(0.5 - w, 0.5 + w, d);
}

@fragment
fn fs_text_color(in: TextOut) -> @location(0) vec4<f32> {
    // Emoji keep their own colors — the tint carries alpha only.
    return textureSample(t_tex, t_samp, in.uv) * u.color;
}
