// One uniform record serves every fragment family (512 B = the dynamic-offset
// stride anyway): mvp + color + a generic payload the family interprets.
// The payload's slots (`PAYLOAD_*`) and the ids the fragments switch on are
// constants generated from `shader_abi.rs`, which declares what each slot
// holds; this module begins with them. Colors are premultiplied everywhere;
// depth (the draw's slot) rides in mvp.

struct DrawUniforms {
    mvp: mat4x4<f32>,
    color: vec4<f32>,
    payload: array<vec4<f32>, PAYLOAD_SLOTS>,
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
    let rect = u.payload[PAYLOAD_RECT];
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

/// Decal coverage for a sampled uv: 0 outside the image on any axis flagged
/// in the decal slot, 1 everywhere else.
///
/// Repeat and mirror ride the sampler's address modes and cost nothing here.
/// Decal cannot: WebGPU has no transparent border colour
/// (`ADDRESS_MODE_CLAMP_TO_BORDER` is outside the baseline), so the sampler
/// clamps and the cutoff happens in the shader. Leaving it out would smear
/// the border texels outwards forever, which is exactly the difference
/// between Canvas2D's `no-repeat` and `repeat`.
fn decal_coverage(uv: vec2<f32>) -> f32 {
    let decal = u.payload[PAYLOAD_DECAL].xy;
    let outside = decal * vec2(f32(uv.x < 0.0 || uv.x > 1.0), f32(uv.y < 0.0 || uv.y > 1.0));
    return 1.0 - min(max(outside.x, outside.y), 1.0);
}

@fragment
fn fs_image(in: VsOut) -> @location(0) vec4<f32> {
    // uv = local × scale + offset (src→dst mapping precomputed CPU-side);
    // tiling comes from the sampler's address modes on out-of-range uv.
    let m = u.payload[PAYLOAD_GEOM];
    let uv = in.local * m.xy + m.zw;
    return textureSample(t_tex, t_samp, uv) * u.color * decal_coverage(uv);
}

// ── gradients (uniform stops, ≤8) ───────────────────────────────────────────

fn stop_offset(i: u32) -> f32 {
    let v = u.payload[PAYLOAD_OFFSETS + (i >> 2u)];
    let lane = i & 3u;
    if lane == 0u { return v.x; }
    if lane == 1u { return v.y; }
    if lane == 2u { return v.z; }
    return v.w;
}

/// Piecewise-linear ramp over straight stop colors. Skia's default and
/// Impeller both interpolate first, then premultiply the resulting color.
fn ramp(t: f32) -> vec4<f32> {
    let count = u32(u.payload[PAYLOAD_MISC].x);
    // Preserve both endpoint colors, as Impeller's CreateGradientBuffer does.
    // Test these before stop intervals: coincident stops can match either interval.
    if t <= 0.0 {
        return u.payload[PAYLOAD_COLORS];
    }
    if t >= 1.0 {
        return u.payload[PAYLOAD_COLORS + count - 1u];
    }
    var prev_off = stop_offset(0u);
    var prev_col = u.payload[PAYLOAD_COLORS];
    if t <= prev_off {
        return prev_col;
    }
    for (var i = 1u; i < count; i = i + 1u) {
        let off = stop_offset(i);
        let col = u.payload[PAYLOAD_COLORS + i];
        if t <= off {
            let span = max(off - prev_off, 1e-6);
            return mix(prev_col, col, (t - prev_off) / span);
        }
        prev_off = off;
        prev_col = col;
    }
    return prev_col;
}

fn premultiply(color: vec4<f32>) -> vec4<f32> {
    return vec4(color.rgb * color.a, color.a);
}

/// Gradients evaluate in their OWN space (Skia's local matrix): the local
/// slots carry the inverse mapping draw-local → gradient coords. Identity
/// for plain gradients — this is a no-op then.
fn gradient_point(p: vec2<f32>) -> vec2<f32> {
    let m = u.payload[PAYLOAD_LOCAL];
    let t = u.payload[PAYLOAD_LOCAL + 1u];
    return vec2(m.x * p.x + m.z * p.y + t.x, m.y * p.x + m.w * p.y + t.y);
}

/// What lives outside 0..1 (misc.z): pad (clamp), repeat (tile), reflect
/// (mirror every other tile). fract() handles negative t for both periodic
/// modes.
fn spread(t: f32) -> f32 {
    let mode = u32(u.payload[PAYLOAD_MISC].z);
    if mode == SPREAD_REPEAT {
        return fract(t);
    }
    if mode == SPREAD_REFLECT {
        let f = fract(t * 0.5) * 2.0;
        return 1.0 - abs(f - 1.0);
    }
    return clamp(t, 0.0, 1.0);
}

fn linear_t(local: vec2<f32>) -> f32 {
    let g = u.payload[PAYLOAD_GEOM]; // (ax, ay, bx, by), gradient space
    let p = gradient_point(local);
    let d = g.zw - g.xy;
    return spread(dot(p - g.xy, d) / max(dot(d, d), 1e-6));
}

@fragment
fn fs_linear(in: VsOut) -> @location(0) vec4<f32> {
    return premultiply(ramp(linear_t(in.local))) * u.color;
}

/// Two-point conical `t`, plus a validity flag: some points of a general
/// conical gradient are covered by NEITHER circle and must stay
/// transparent. The conical slot holds `(kind, r1_in_unit_space, focal,
/// sign)` and the flags slot `(swapped, focal_on_circle, well_behaved, _)`,
/// both settled on the CPU — the position arriving here is already in focal
/// space when the general case needs it.
fn radial_t(local: vec2<f32>) -> vec2<f32> {
    let setup = u.payload[PAYLOAD_CONICAL];
    let kind = setup.x;
    let p = gradient_point(local);

    // Concentric: t is just the fraction of the way between the two radii.
    if kind == CONICAL_CONCENTRIC {
        let g = u.payload[PAYLOAD_GEOM];
        let start_radius = setup.y;
        let end_radius = setup.z;
        let distance = length(p - g.xy);
        return vec2(spread((distance - start_radius) / (end_radius - start_radius)), 1.0);
    }
    // Identical circles paint nothing at all.
    if kind == CONICAL_EMPTY {
        return vec2(0.0, 0.0);
    }
    // Equal radii: the gradient sweeps the strip between the circles' common
    // tangents, and everything beyond those tangents is uncovered.
    if kind == CONICAL_STRIP {
        let radius_squared = setup.y;
        let half_span = radius_squared - p.y * p.y;
        if half_span < 0.0 {
            return vec2(0.0, 0.0);
        }
        return vec2(spread(p.x + sqrt(half_span)), 1.0);
    }

    // The general case, continuing Skia's algorithm from step 5.
    let flags = u.payload[PAYLOAD_CONICAL_FLAGS];
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
    return premultiply(ramp(solved.x)) * u.color * solved.y;
}

const TAU: f32 = 6.28318530718;

fn sweep_t(local: vec2<f32>) -> f32 {
    let g = u.payload[PAYLOAD_GEOM]; // (cx, cy, _, _); start angle in misc.y
    let v = gradient_point(local) - g.xy;
    return fract((atan2(v.y, v.x) - u.payload[PAYLOAD_MISC].y) / TAU);
}

@fragment
fn fs_sweep(in: VsOut) -> @location(0) vec4<f32> {
    return premultiply(ramp(sweep_t(in.local))) * u.color;
}

// ── ramp gradients (>8 stops): Impeller's texture path ──────────────────────
// The stop list lives in a baked N×1 straight-color texture; misc.x
// carries N so t maps to texel CENTERS (linear filtering interpolates
// between baked samples; hard stops can soften within one texel interval).

fn sample_ramp(t: f32) -> vec4<f32> {
    let n = u.payload[PAYLOAD_MISC].x;
    let uv = vec2((t * (n - 1.0) + 0.5) / n, 0.5);
    return textureSample(t_tex, t_samp, uv);
}

@fragment
fn fs_linear_ramp(in: VsOut) -> @location(0) vec4<f32> {
    return premultiply(sample_ramp(linear_t(in.local))) * u.color;
}

@fragment
fn fs_radial_ramp(in: VsOut) -> @location(0) vec4<f32> {
    let solved = radial_t(in.local);
    return premultiply(sample_ramp(solved.x)) * u.color * solved.y;
}

@fragment
fn fs_sweep_ramp(in: VsOut) -> @location(0) vec4<f32> {
    return premultiply(sample_ramp(sweep_t(in.local))) * u.color;
}

// ── mask composite ──────────────────────────────────────────────────────────
// The mask layer's texture as COVERAGE, drawn with DstIn over the whole
// enclosing layer. The geom slot maps local → mask uv; misc.x picks
// luminance or alpha. Outside the mask texture coverage is 0 —
// that erasure of unmasked content is the point (never clamp-smear edge
// texels outward).

@fragment
fn fs_mask_composite(in: VsOut) -> @location(0) vec4<f32> {
    let m = u.payload[PAYLOAD_GEOM];
    let uv = in.local * m.xy + m.zw;
    let s = textureSample(t_tex, t_samp, clamp(uv, vec2(0.0), vec2(1.0)));
    let inside = f32(all(uv >= vec2(0.0)) && all(uv <= vec2(1.0)));
    var coverage = s.a;
    if u.payload[PAYLOAD_MISC].x == MASK_LUMINANCE {
        // Premultiplied luma = luma(straight) × alpha in one dot (BT.709).
        coverage = dot(s.rgb, vec3(0.2126, 0.7152, 0.0722));
    }
    return vec4(0.0, 0.0, 0.0, coverage * inside * u.color.a);
}

// ── advanced (dst-reading) blends ───────────────────────────────────────────
// The pass broke before these draws: `t_dst` holds a snapshot of the target,
// sampled at framebuffer coords (uv = position / misc.zw). The shader
// computes blend + composite in one (PDF/W3C compositing formulas over
// UNpremultiplied color); pipeline blending is OFF — output replaces dst.
// The mode in misc.x is one of the `ADVANCED_*` ids.

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
        c = l + (c - l) * l / (l - n + 1e-3);
    }
    if x > 1.0 {
        c = l + (c - l) * (1.0 - l) / (x - l + 1e-3);
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
    let r = min(vec3(1.0), d / max(1.0 - s, vec3(1e-3)));
    return select(select(r, vec3(1.0), 1.0 - s < vec3(1e-3)), vec3(0.0), d < vec3(1e-3));
}

fn color_burn(s: vec3<f32>, d: vec3<f32>) -> vec3<f32> {
    let r = 1.0 - min(vec3(1.0), (1.0 - d) / max(s, vec3(1e-3)));
    return select(select(r, vec3(0.0), s < vec3(1e-3)), vec3(1.0), 1.0 - d < vec3(1e-3));
}

fn blend_advanced(mode: u32, s: vec3<f32>, d: vec3<f32>) -> vec3<f32> {
    switch mode {
        case ADVANCED_MULTIPLY: { return s * d; }                       // Multiply
        case ADVANCED_OVERLAY: { return hard_light(d, s); }            // Overlay
        case ADVANCED_DARKEN: { return min(s, d); }                   // Darken
        case ADVANCED_LIGHTEN: { return max(s, d); }                   // Lighten
        case ADVANCED_COLOR_DODGE: { return color_dodge(s, d); }           // ColorDodge
        case ADVANCED_COLOR_BURN: { return color_burn(s, d); }            // ColorBurn
        case ADVANCED_HARD_LIGHT: { return hard_light(s, d); }            // HardLight
        case ADVANCED_SOFT_LIGHT: { return soft_light(s, d); }            // SoftLight
        case ADVANCED_DIFFERENCE: { return abs(s - d); }                  // Difference
        case ADVANCED_EXCLUSION: { return s + d - 2.0 * s * d; }         // Exclusion
        case ADVANCED_HUE: { return set_lum(set_sat(s, sat(d)), lum(d)); } // Hue
        case ADVANCED_SATURATION: { return set_lum(set_sat(d, sat(s)), lum(d)); } // Saturation
        case ADVANCED_COLOR: { return set_lum(s, lum(d)); }         // Color
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
    let uv = pos.xy / u.payload[PAYLOAD_MISC].zw; // target size in misc.zw
    return textureSample(t_tex, t_samp, uv);
}

/// Solid src × snapshot dst (group1 texture = the dst snapshot).
@fragment
fn fs_blend_solid(in: VsOut) -> @location(0) vec4<f32> {
    let mode = u32(u.payload[PAYLOAD_MISC].x);
    return composite_advanced(mode, u.color, dst_sample(in.pos));
}

// Texture src (a layer or desugared draw) × snapshot dst.
@group(1) @binding(2) var t_src: texture_2d<f32>;

@fragment
fn fs_blend_texture(in: VsOut) -> @location(0) vec4<f32> {
    let m = u.payload[PAYLOAD_GEOM];
    let src_uv = in.local * m.xy + m.zw;
    let src = textureSample(t_src, t_samp, src_uv) * u.color;
    let mode = u32(u.payload[PAYLOAD_MISC].x);
    return composite_advanced(mode, src, dst_sample(in.pos));
}

// ── patterns ────────────────────────────────────────────────────────────────
// An image tiled across the shape. `gradient_point` already carries the local
// position through the paint's inverse local matrix, so all that remains is
// pattern pixels → uv. Tiling rides the sampler's address modes and
// `decal_coverage`, exactly as an image DRAW's does.

@fragment
fn fs_pattern(in: VsOut) -> @location(0) vec4<f32> {
    let uv = gradient_point(in.local) * u.payload[PAYLOAD_GEOM].xy;
    return textureSample(t_tex, t_samp, uv) * u.color * decal_coverage(uv);
}

// ── colour filters (filter passes only) ─────────────────────────────────────
// Run over a snapshot when a filter's result has to be a texture. The geom
// slot maps this pass's local space to source uv, exactly as the blur passes
// do. misc.w is the alpha the input is taken in at before the filter
// (Impeller's `input_alpha`, which absorbs a layer's opacity).

/// The colour-matrix slots hold the 4×5's rows, then its translation column.
@fragment
fn fs_color_matrix(in: VsOut) -> @location(0) vec4<f32> {
    let m = u.payload[PAYLOAD_GEOM];
    let texel = textureSample(t_tex, t_samp, in.local * m.xy + m.zw);
    return apply_color_matrix(texel * u.payload[PAYLOAD_MISC].w);
}

fn apply_color_matrix(texel: vec4<f32>) -> vec4<f32> {
    // Colour matrices are defined on STRAIGHT colour; layers are premultiplied.
    let color = vec4(unpremul(texel), texel.a);
    let filtered = clamp(
        vec4(
            dot(u.payload[PAYLOAD_COLOR_MATRIX], color),
            dot(u.payload[PAYLOAD_COLOR_MATRIX + 1u], color),
            dot(u.payload[PAYLOAD_COLOR_MATRIX + 2u], color),
            dot(u.payload[PAYLOAD_COLOR_MATRIX + 3u], color),
        ) + u.payload[PAYLOAD_COLOR_MATRIX + 4u],
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
        case BLEND_CLEAR: {}                                        // Clear
        case BLEND_SRC: { fs = 1.0; }                             // Src
        case BLEND_DST: { fd = 1.0; }                             // Dst
        case BLEND_SRC_OVER: { fs = 1.0; fd = 1.0 - src.a; }           // SrcOver
        case BLEND_DST_OVER: { fs = 1.0 - dst.a; fd = 1.0; }           // DstOver
        case BLEND_SRC_IN: { fs = dst.a; }                           // SrcIn
        case BLEND_DST_IN: { fd = src.a; }                           // DstIn
        case BLEND_SRC_OUT: { fs = 1.0 - dst.a; }                     // SrcOut
        case BLEND_DST_OUT: { fd = 1.0 - src.a; }                     // DstOut
        case BLEND_SRC_ATOP: { fs = dst.a; fd = 1.0 - src.a; }         // SrcAtop
        case BLEND_DST_ATOP: { fs = 1.0 - dst.a; fd = src.a; }        // DstAtop
        case BLEND_XOR: { fs = 1.0 - dst.a; fd = 1.0 - src.a; }  // Xor
        case BLEND_PLUS: { return min(src + dst, vec4(1.0)); }    // Plus (clamped)
        case BLEND_MODULATE: { return src * dst; }                    // Modulate
        default: { return src + dst - src * dst; }         // Screen
    }
    return src * fs + dst * fd;
}

/// A constant colour blended AS THE SOURCE over the layer — Flutter's
/// `ColorFilter.mode`. The colour-matrix slot holds that colour
/// premultiplied; misc.x the mode, a `BLEND_*` id or an `ADVANCED_*` one
/// past `ADVANCED_FILTER_BASE`.
@fragment
fn fs_color_blend(in: VsOut) -> @location(0) vec4<f32> {
    let m = u.payload[PAYLOAD_GEOM];
    let dst = textureSample(t_tex, t_samp, in.local * m.xy + m.zw);
    return apply_color_blend(dst * u.payload[PAYLOAD_MISC].w);
}

fn apply_color_blend(dst: vec4<f32>) -> vec4<f32> {
    let src = u.payload[PAYLOAD_COLOR_MATRIX];
    let mode = u32(u.payload[PAYLOAD_MISC].x);
    if mode >= ADVANCED_FILTER_BASE {
        return composite_advanced(mode - ADVANCED_FILTER_BASE, src, dst);
    }
    return composite_porter_duff(mode, src, dst);
}

/// The sRGB transfer curve in either direction (misc.x) — Flutter's
/// `ColorFilter.linearToSrgbGamma` and `ColorFilter.srgbToLinearGamma`.
@fragment
fn fs_color_gamma(in: VsOut) -> @location(0) vec4<f32> {
    let m = u.payload[PAYLOAD_GEOM];
    let texel = textureSample(t_tex, t_samp, in.local * m.xy + m.zw);
    return apply_gamma(texel * u.payload[PAYLOAD_MISC].w);
}

/// Impeller's linear_to_srgb_filter.frag and srgb_to_linear_filter.frag: the
/// curve is defined on STRAIGHT colour, so unpremultiply, curve each channel,
/// premultiply. Alpha passes through.
fn apply_gamma(texel: vec4<f32>) -> vec4<f32> {
    let color = unpremul(texel);
    var curved: vec3<f32>;
    if u.payload[PAYLOAD_MISC].x == GAMMA_LINEAR_TO_SRGB {
        curved = vec3(linear_to_srgb(color.r), linear_to_srgb(color.g), linear_to_srgb(color.b));
    } else {
        curved = vec3(srgb_to_linear(color.r), srgb_to_linear(color.g), srgb_to_linear(color.b));
    }
    return vec4(curved * texel.a, texel.a);
}

// The branches keep `pow`'s base positive, where WGSL defines it.
fn linear_to_srgb(channel: f32) -> f32 {
    if channel <= 0.0031308 {
        return channel * 12.92;
    }
    return 1.055 * pow(channel, 1.0 / 2.4) - 0.055;
}

fn srgb_to_linear(channel: f32) -> f32 {
    if channel <= 0.04045 {
        return channel / 12.92;
    }
    return pow((channel + 0.055) / 1.055, 2.4);
}

// A texture drawn through a colour filter: an image draw, its filter after
// crop/tile/filter/mipmap sampling like Impeller's ColorFilterAtlasContents,
// or a filtered layer's composite, Impeller's colour filter drawing its input
// snapshot. misc.w is the alpha the texel is taken in at before the
// filter and u.color the alpha after it (Impeller's `input_alpha` and
// `output_alpha`): an image keeps its paint alpha last, a layer's composite
// takes its alpha in first.
@fragment
fn fs_image_matrix(in: VsOut) -> @location(0) vec4<f32> {
    let m = u.payload[PAYLOAD_GEOM];
    let uv = in.local * m.xy + m.zw;
    let texel = textureSample(t_tex, t_samp, uv);
    return apply_color_matrix(texel * u.payload[PAYLOAD_MISC].w) * u.color * decal_coverage(uv);
}

@fragment
fn fs_image_blend(in: VsOut) -> @location(0) vec4<f32> {
    let m = u.payload[PAYLOAD_GEOM];
    let uv = in.local * m.xy + m.zw;
    let texel = textureSample(t_tex, t_samp, uv);
    return apply_color_blend(texel * u.payload[PAYLOAD_MISC].w) * u.color * decal_coverage(uv);
}

@fragment
fn fs_image_gamma(in: VsOut) -> @location(0) vec4<f32> {
    let m = u.payload[PAYLOAD_GEOM];
    let uv = in.local * m.xy + m.zw;
    let texel = textureSample(t_tex, t_samp, uv);
    return apply_gamma(texel * u.payload[PAYLOAD_MISC].w) * u.color * decal_coverage(uv);
}

// ── gaussian blur, one direction ────────────────────────────────────────────
// Impeller's gaussian.frag: one pass of a separable blur over a texture the
// size of the target. The kernel — Impeller's KernelSamples block, merged so
// that most samples are one bilinear fetch between two texels — rides the
// host buffer at group 2, at a dynamic offset of its own: (uv offset,
// coefficient, _) each.
// The geom slot maps the pass to uv; misc = (sample count, divide the
// result by its alpha, _, _), the division being a bounded blur's last pass.

struct KernelSamples {
    sample_data: array<vec4<f32>, MAX_KERNEL_SAMPLES>,
};

@group(2) @binding(0) var<uniform> kernel_samples: KernelSamples;

@fragment
fn fs_blur(in: VsOut) -> @location(0) vec4<f32> {
    let m = u.payload[PAYLOAD_GEOM];
    let uv = in.local * m.xy + m.zw;
    let sample_count = i32(u.payload[PAYLOAD_MISC].x);
    var total = vec4(0.0);
    for (var i = 0; i < sample_count; i = i + 1) {
        let sample = kernel_samples.sample_data[i];
        total += sample.z * textureSample(t_tex, t_samp, uv + sample.xy);
    }
    if u.payload[PAYLOAD_MISC].y > 0.5 {
        return unpremultiply_opaque(total);
    }
    return total;
}

/// Impeller's IPHalfUnpremultiplyOpaque: every channel over alpha, so alpha
/// becomes 1; transparent stays transparent.
fn unpremultiply_opaque(color: vec4<f32>) -> vec4<f32> {
    if color.a == 0.0 {
        return vec4(0.0);
    }
    return color / color.a;
}

// ── blur downsample ─────────────────────────────────────────────────────────
// Impeller's downsample pass: texture_fill.frag's one bilinear tap (edge 0,
// ratio 1), or downsample.glsl's taps at odd texel offsets out to `edge`.
// The geom slot maps the pass to source uv; misc = (edge, ratio, texel size
// in uv). The downsample modes slot = (bounded, decal, width, height): a
// bounded blur's taps outside the four edge slots — its quad's edges as
// (a, b, c, _) in source uv, inside where every a·u + b·v + c ≥ 0 — read
// transparent (texture_downsample_bounded.frag). A tap past the texture's
// edge reads as the sampler's address mode says, the blur's tile mode. A
// decal one reads as Impeller's decal sampler does on Metal and Vulkan, a
// transparent border past the edge: WebGPU has none, so the sampler clamps
// and the tap is weighted by the share of its bilinear footprint inside the
// texture, on the grid of the mip level it reads, width × height texels.

@fragment
fn fs_downsample(in: VsOut) -> @location(0) vec4<f32> {
    let m = u.payload[PAYLOAD_GEOM];
    let uv = in.local * m.xy + m.zw;
    let edge = u.payload[PAYLOAD_MISC].x;
    let ratio = u.payload[PAYLOAD_MISC].y;
    let pixel_size = u.payload[PAYLOAD_MISC].zw;
    var total = vec4(0.0);
    for (var i = -edge; i <= edge; i = i + 2.0) {
        for (var j = -edge; j <= edge; j = j + 2.0) {
            total += downsample_tap(uv + pixel_size * vec2(i, j)) * ratio;
        }
    }
    return total;
}

/// One tap of the downsample. The sample is taken either way: sampling
/// under a non-uniform branch is invalid WGSL.
fn downsample_tap(uv: vec2<f32>) -> vec4<f32> {
    let texel = textureSample(t_tex, t_samp, uv);
    return texel * decal_border(uv) * bounds_inside(uv);
}

/// The share of a tap's bilinear footprint, one texel square centred on
/// it, that lies inside the texture: what a transparent border leaves of a
/// clamped tap. A tap on the edge reads half the edge texel; one a texel
/// past it, nothing. Testing the tap against the edge instead would read
/// the edge texel whole or not at all as the tap's last bit fell, and a
/// downsample whose taps land on both edges would move its blur by half a
/// texel.
fn decal_border(uv: vec2<f32>) -> f32 {
    let modes = u.payload[PAYLOAD_DOWNSAMPLE_MODES];
    if modes.y < 0.5 {
        return 1.0;
    }
    let texels = modes.zw;
    let position = uv * texels;
    let inside = clamp(min(position + 0.5, texels + 0.5 - position), vec2(0.0), vec2(1.0));
    return inside.x * inside.y;
}

fn bounds_inside(uv: vec2<f32>) -> f32 {
    if u.payload[PAYLOAD_DOWNSAMPLE_MODES].x < 0.5 {
        return 1.0;
    }
    let point = vec4(uv, 1.0, 0.0);
    let distances = vec4(
        dot(point, u.payload[PAYLOAD_DOWNSAMPLE_EDGES]),
        dot(point, u.payload[PAYLOAD_DOWNSAMPLE_EDGES + 1u]),
        dot(point, u.payload[PAYLOAD_DOWNSAMPLE_EDGES + 2u]),
        dot(point, u.payload[PAYLOAD_DOWNSAMPLE_EDGES + 3u]),
    );
    return f32(all(distances >= vec4(0.0)));
}

// ── closed-form blurred rounded rect (Impeller rrect_blur) ──────────────────
// Why a box shadow is ONE draw: a 2-D gaussian ∗ box separates into 1-D
// convolutions, and a blurred step edge is an erf. Exact along x, 4-sample
// gauss-weighted integration along y where the rounded profile varies —
// Evan Wallace's "fast rounded rectangle shadows" formulation, generalized
// to PER-CORNER radii (each row's left/right bound uses its own corner).
// geom = rrect (x0, y0, x1, y1) local; misc = (sigma, style, _, _); the
// radii slot = (tl, tr, br, bl). Styles are the `STYLE_*` ids.

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
        case STYLE_SOLID: { return sharp + blurred * (1.0 - sharp); }
        case STYLE_INNER: { return blurred * sharp; }
        case STYLE_OUTER: { return blurred * (1.0 - sharp); }
        default: { return blurred; }
    }
}

@fragment
fn fs_rrect_blur(in: VsOut) -> @location(0) vec4<f32> {
    let r = u.payload[PAYLOAD_GEOM];
    let sigma = max(u.payload[PAYLOAD_MISC].x, 0.05);
    let style = u32(u.payload[PAYLOAD_MISC].y);
    let radii = u.payload[PAYLOAD_RADII];
    let half_size = (r.zw - r.xy) * 0.5;
    let p = in.local - (r.xy + r.zw) * 0.5;
    let blurred = rrect_blur_coverage(p, half_size, sigma, radii);
    // Screen-space AA on the sharp edge (fwidth = local units per pixel).
    let d = rrect_sdf(p, half_size, radii);
    let sharp = clamp(0.5 - d / max(fwidth(d), 1e-4), 0.0, 1.0);
    return u.color * styled_coverage(style, blurred, sharp);
}

// ── two-input merges: blur styles and drop shadows ─────────────────────────
// A quad over the union of what two snapshots cover, each sampled through its
// own transform: the geom slot maps the quad's local space to the first's
// uv (t_tex), the second uv slot to the second's (t_src); misc.x is the
// fragment's switch, u.color the alpha the result is drawn at. A filter's last node
// draws it straight into the destination; a pass makes it a texture when
// something reads it next. Outside a snapshot its sample reads transparent,
// so a clamping sampler never smears its edge texels across the quad.

fn sample_first(local: vec2<f32>) -> vec4<f32> {
    let m = u.payload[PAYLOAD_GEOM];
    let uv = local * m.xy + m.zw;
    return textureSample(t_tex, t_samp, uv) * inside_unit(uv);
}

fn sample_second(local: vec2<f32>) -> vec4<f32> {
    let m = u.payload[PAYLOAD_SECOND_UV];
    let uv = local * m.xy + m.zw;
    return textureSample(t_src, t_samp, uv) * inside_unit(uv);
}

fn inside_unit(uv: vec2<f32>) -> f32 {
    return f32(all(uv >= vec2(0.0)) && all(uv <= vec2(1.0)));
}

// Blur style combine: merges the blurred layer B (first) with the SHARP
// layer M (second) so the composite stays one texture for any blend mode.
// misc.x = the style.
@fragment
fn fs_mask_combine(in: VsOut) -> @location(0) vec4<f32> {
    let b = sample_first(in.local);
    let m = sample_second(in.local);
    let style = u32(u.payload[PAYLOAD_MISC].x);
    switch style {
        case STYLE_SOLID: { return (m + b * (1.0 - m.a)) * u.color; }
        case STYLE_INNER: { return b * m.a * u.color; }
        default: { return b * (1.0 - m.a) * u.color; }
    }
}

// Drop-shadow merge: the sharp layer S (second) over its recoloured, blurred
// and moved copy B (first), Skia's `Merge`.
@fragment
fn fs_drop_shadow(in: VsOut) -> @location(0) vec4<f32> {
    let b = sample_first(in.local);
    let s = sample_second(in.local);
    return (s + b * (1.0 - s.a)) * u.color;
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

// Colour glyphs through the run's colour filter, as Skia's colour-bitmap
// text runs it on the glyph's pixel at the paint's alpha: misc.w is that
// alpha, taken in before the filter, and u.color the group alpha after it.
@fragment
fn fs_text_color_matrix(in: TextOut) -> @location(0) vec4<f32> {
    let texel = textureSample(t_tex, t_samp, in.uv);
    return apply_color_matrix(texel * u.payload[PAYLOAD_MISC].w) * u.color;
}

@fragment
fn fs_text_color_blend(in: TextOut) -> @location(0) vec4<f32> {
    let texel = textureSample(t_tex, t_samp, in.uv);
    return apply_color_blend(texel * u.payload[PAYLOAD_MISC].w) * u.color;
}

@fragment
fn fs_text_color_gamma(in: TextOut) -> @location(0) vec4<f32> {
    let texel = textureSample(t_tex, t_samp, in.uv);
    return apply_gamma(texel * u.payload[PAYLOAD_MISC].w) * u.color;
}
