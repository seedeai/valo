// Copyright 2014 The Flutter Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

// Material's ink sparkle, ported from Flutter's
// packages/flutter/lib/src/material/shaders/ink_sparkle.frag as a valo paint shader. The
// uniforms keep the GLSL file's names and order; a host writing Flutter's tightly packed float
// indices repacks them into this struct's WGSL layout (u_resolution_scale, float 11, is at
// byte 48). GLSL's `mod` floors where WGSL's `%` truncates, so `floor_mod` stands in for it.

struct Uniforms {
    u_color: vec4<f32>,
    // u_alpha, u_sparkle_alpha, u_blur, u_radius_scale
    u_composite_1: vec4<f32>,
    u_center: vec2<f32>,
    u_max_radius: f32,
    u_resolution_scale: vec2<f32>,
    u_noise_scale: vec2<f32>,
    u_noise_phase: f32,
    u_circle1: vec2<f32>,
    u_circle2: vec2<f32>,
    u_circle3: vec2<f32>,
    u_rotation1: vec2<f32>,
    u_rotation2: vec2<f32>,
    u_rotation3: vec2<f32>,
}
@group(2) @binding(0) var<uniform> u: Uniforms;

const PI: f32 = 3.1415926535897932384626;
const TURBULENCE_SCALE: vec2<f32> = vec2<f32>(0.8);

fn floor_mod(x: vec2<f32>, y: vec2<f32>) -> vec2<f32> {
    return x - y * floor(x / y);
}

fn triangle_noise(n_in: vec2<f32>) -> f32 {
    var n = fract(n_in * vec2<f32>(5.3987, 5.4421));
    n += dot(n.yx, n.xy + vec2<f32>(21.5351, 14.3137));
    let xy = n.x * n.y;
    return fract(xy * 95.4307) + fract(xy * 75.04961) - 1.0;
}

fn threshold(v: f32, l: f32, h: f32) -> f32 {
    return step(l, v) * (1.0 - step(h, v));
}

fn rotate2d(rad: vec2<f32>) -> mat2x2<f32> {
    return mat2x2<f32>(rad.x, -rad.y, rad.y, rad.x);
}

fn soft_circle(uv: vec2<f32>, xy: vec2<f32>, radius: f32, blur: f32) -> f32 {
    let blur_half = blur * 0.5;
    let d = distance(uv, xy);
    return 1.0 - smoothstep(1.0 - blur_half, 1.0 + blur_half, d / radius);
}

fn soft_ring(uv: vec2<f32>, xy: vec2<f32>, radius: f32, thickness: f32, blur: f32) -> f32 {
    let circle_outer = soft_circle(uv, xy, radius + thickness, blur);
    let circle_inner = soft_circle(uv, xy, max(radius - thickness, 0.0), blur);
    return clamp(circle_outer - circle_inner, 0.0, 1.0);
}

fn circle_grid(
    resolution: vec2<f32>,
    p_in: vec2<f32>,
    xy: vec2<f32>,
    rotation: vec2<f32>,
    cell_diameter: f32,
) -> f32 {
    var p = rotate2d(rotation) * (xy - p_in) + xy;
    p = floor_mod(p, vec2<f32>(cell_diameter)) / resolution;
    let cell_uv = cell_diameter / resolution.y * 0.5;
    let r = 0.65 * cell_uv;
    return soft_circle(p, vec2<f32>(cell_uv), r, r * 50.0);
}

fn sparkle(uv: vec2<f32>, t: f32) -> f32 {
    let n = triangle_noise(uv);
    var s = threshold(n, 0.0, 0.05);
    s += threshold(n + sin(PI * (t + 0.35)), 0.1, 0.15);
    s += threshold(n + sin(PI * (t + 0.7)), 0.2, 0.25);
    s += threshold(n + sin(PI * (t + 1.05)), 0.3, 0.35);
    return clamp(s, 0.0, 1.0) * 0.55;
}

fn turbulence(uv: vec2<f32>) -> f32 {
    let uv_scale = uv * TURBULENCE_SCALE;
    let g1 = circle_grid(TURBULENCE_SCALE, uv_scale, u.u_circle1, u.u_rotation1, 0.17);
    let g2 = circle_grid(TURBULENCE_SCALE, uv_scale, u.u_circle2, u.u_rotation2, 0.2);
    let g3 = circle_grid(TURBULENCE_SCALE, uv_scale, u.u_circle3, u.u_rotation3, 0.275);
    let v = (g1 * g1 + g2 - g3) * 0.5;
    return clamp(0.45 + 0.8 * v, 0.0, 1.0);
}

fn shade(position: vec2<f32>) -> vec4<f32> {
    let u_alpha = u.u_composite_1.x;
    let u_sparkle_alpha = u.u_composite_1.y;
    let u_blur = u.u_composite_1.z;
    let u_radius_scale = u.u_composite_1.w;

    let p = position;
    let uv = p * u.u_resolution_scale;
    let density_uv = uv - floor_mod(p, u.u_noise_scale);
    let radius = u.u_max_radius * u_radius_scale;
    let turbulence_value = turbulence(uv);
    let ring = soft_ring(p, u.u_center, radius, 0.05 * u.u_max_radius, u_blur);
    let sparkle_value =
        sparkle(density_uv, u.u_noise_phase) * ring * turbulence_value * u_sparkle_alpha;
    let wave_alpha = soft_circle(p, u.u_center, radius, u_blur) * u_alpha * u.u_color.a;
    let wave_color = vec4<f32>(u.u_color.rgb * wave_alpha, wave_alpha);
    return mix(wave_color, vec4<f32>(1.0), sparkle_value);
}
