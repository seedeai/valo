// The draw record and vertex stages every module shares: `solid.wgsl` and each runtime
// effect's generated module begin with it, after the ABI prelude. The record is `valo_draw` so
// an effect author's names cannot collide with it.

struct DrawUniforms {
    mvp: mat4x4<f32>,
    color: vec4<f32>,
    payload: array<vec4<f32>, PAYLOAD_SLOTS>,
};

@group(0) @binding(0) var<uniform> valo_draw: DrawUniforms;

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
    let rect = valo_draw.payload[PAYLOAD_RECT];
    var out: VsOut;
    out.pos = valo_draw.mvp * vec4<f32>(corner, 0.0, 1.0);
    out.local = rect.xy + corner * rect.zw;
    return out;
}

@vertex
fn vs_mesh(@location(0) p: vec2<f32>) -> VsOut {
    var out: VsOut;
    out.pos = valo_draw.mvp * vec4<f32>(p, 0.0, 1.0);
    out.local = p;
    return out;
}
