//! The shader ABI: the uniform record every fragment of
//! `shaders/solid.wgsl` reads — its transform, its tint, and the payload
//! slots each fragment family interprets — and the ids the fragments'
//! switches take. The WGSL reads the same numbers from a prelude generated
//! from the constants here ([`wgsl_prelude`]), so the two cannot drift: a
//! slot or an id is named once, in Rust.
//!
//! Colours are premultiplied everywhere; a draw's depth rides in its
//! transform.

use valo_dl::{BlurStyle, MaskKind, SpreadMode};

use crate::host_buffer::UNIFORM_SIZE;
use crate::pipelines::{AdvancedBlend, PipelineBlend};

/// `UniformRecord` is one draw's uniform block: its transform, its tint and
/// its payload slots.
#[derive(Clone)]
pub(crate) struct UniformRecord {
    bytes: [u8; UNIFORM_SIZE as usize],
}

/// Where the record's parts start, in bytes.
const MVP_BYTES: usize = 0;
const TINT_BYTES: usize = 64;
const PAYLOAD_BYTES: usize = 80;

/// `PAYLOAD_SLOTS` is how many `vec4` payload slots a record holds.
pub(crate) const PAYLOAD_SLOTS: usize = (UNIFORM_SIZE as usize - PAYLOAD_BYTES) / 16;

// The last slots, the colour matrix's five, fit the record.
const _: () = assert!(slot::COLOR_MATRIX + 5 <= PAYLOAD_SLOTS);

impl UniformRecord {
    /// `tinted` is an empty record whose output is multiplied by `tint`.
    pub fn tinted(tint: [f32; 4]) -> Self {
        let mut record = Self {
            bytes: [0; UNIFORM_SIZE as usize],
        };
        record.bytes[TINT_BYTES..TINT_BYTES + 16].copy_from_slice(bytemuck::cast_slice(&tint));
        record
    }

    /// `set_mvp` places the draw: model, view and projection, the draw's
    /// depth folded in.
    pub fn set_mvp(&mut self, mvp: [f32; 16]) {
        self.bytes[MVP_BYTES..MVP_BYTES + 64].copy_from_slice(bytemuck::cast_slice(&mvp));
    }

    /// `set` writes payload slot `slot`.
    pub fn set(&mut self, slot: usize, value: [f32; 4]) {
        let start = PAYLOAD_BYTES + slot * 16;
        self.bytes[start..start + 16].copy_from_slice(bytemuck::cast_slice(&value));
    }

    pub fn bytes(&self) -> &[u8] {
        &self.bytes
    }
}

/// The payload slots, by what reads them.
pub(crate) mod slot {
    /// The draw's local-space rect `(x, y, w, h)`: `vs_quad` derives the
    /// local varying from it, so gradients and images live in draw space.
    pub const RECT: usize = 0;
    /// The family's geometry: an image's or a texture's uv mapping
    /// `uv = p · xy + zw`, a gradient's points, a rrect's edges, a pass's
    /// texel size.
    pub const GEOM: usize = 1;
    /// The family's switches and scalars: a gradient's stop count, angle,
    /// spread and focal y; a filter's mode and input alpha; a blur's sample
    /// count and division; a downsample's taps; a blend's mode and the
    /// target's size.
    pub const MISC: usize = 2;
    /// Two slots of 8 gradient stop offsets.
    pub const OFFSETS: usize = 3;
    /// A blurred rrect's corner radii (no gradient there).
    pub const RADII: usize = 3;
    /// Per-axis decal flags of an image or pattern (no gradient there).
    pub const DECAL: usize = 3;
    /// Four slots: a bounded downsample's quad edges, in uv.
    pub const DOWNSAMPLE_EDGES: usize = 3;
    /// A downsample's `(bounded, decal, _, _)`.
    pub const DOWNSAMPLE_MODES: usize = 8;
    /// A two-input fragment's second uv mapping.
    pub const SECOND_UV: usize = 3;
    /// Eight slots of gradient stop colours, straight.
    pub const COLORS: usize = 5;
    /// Two slots: the inverse gradient or pattern local matrix
    /// `(a, b, c, d | tx, ty, _, _)`.
    pub const LOCAL: usize = 13;
    /// A two-point conical gradient's case and constants.
    pub const CONICAL: usize = 15;
    /// A two-point conical gradient's `(swapped, focal on circle, well
    /// behaved, _)`.
    pub const CONICAL_FLAGS: usize = 16;
    /// Five slots: a colour matrix's rows and its translation column; the
    /// first alone carries a blend filter's premultiplied source colour.
    pub const COLOR_MATRIX: usize = 17;
}

/// A gradient's spread past 0..1.
pub(crate) fn spread_id(spread: SpreadMode) -> f32 {
    match spread {
        SpreadMode::Pad => 0.0,
        SpreadMode::Repeat => 1.0,
        SpreadMode::Reflect => 2.0,
    }
}

/// A two-point conical gradient's cases, solved on the CPU.
pub(crate) mod conical {
    pub const CONCENTRIC: f32 = 0.0;
    pub const GENERAL: f32 = 1.0;
    pub const EMPTY: f32 = 2.0;
    pub const STRIP: f32 = 3.0;
}

/// The sRGB curve's directions.
pub(crate) mod gamma {
    pub const SRGB_TO_LINEAR: f32 = 0.0;
    pub const LINEAR_TO_SRGB: f32 = 1.0;
}

/// `blur_style_id` is the switch a blurred rrect and a blur style merge
/// take.
pub(crate) fn blur_style_id(style: BlurStyle) -> f32 {
    match style {
        BlurStyle::Normal => 0.0,
        BlurStyle::Solid => 1.0,
        BlurStyle::Inner => 2.0,
        BlurStyle::Outer => 3.0,
    }
}

/// `mask_kind_id` is the switch the mask composite takes: luminance or
/// alpha coverage.
pub(crate) fn mask_kind_id(kind: MaskKind) -> f32 {
    match kind {
        MaskKind::Alpha => 0.0,
        MaskKind::Luminance => 1.0,
    }
}

/// `ADVANCED_FILTER_BASE` is where a blend colour filter's advanced modes
/// start among its ids, after the pipeline blends'.
pub(crate) const ADVANCED_FILTER_BASE: u32 = 15;

/// `wgsl_prelude` declares the ABI's slots and ids as WGSL constants, for
/// the shader module to begin with.
pub(crate) fn wgsl_prelude() -> String {
    let mut prelude = String::from("// Generated from shader_abi.rs: payload slots and ids.\n");
    let mut line = |name: &str, value: String| {
        prelude.push_str(&format!("const {name} = {value};\n"));
    };
    let u32_value = |value: usize| format!("{value}u");
    line("PAYLOAD_SLOTS", u32_value(PAYLOAD_SLOTS));
    for (name, value) in [
        ("PAYLOAD_RECT", slot::RECT),
        ("PAYLOAD_GEOM", slot::GEOM),
        ("PAYLOAD_MISC", slot::MISC),
        ("PAYLOAD_OFFSETS", slot::OFFSETS),
        ("PAYLOAD_RADII", slot::RADII),
        ("PAYLOAD_DECAL", slot::DECAL),
        ("PAYLOAD_DOWNSAMPLE_EDGES", slot::DOWNSAMPLE_EDGES),
        ("PAYLOAD_DOWNSAMPLE_MODES", slot::DOWNSAMPLE_MODES),
        ("PAYLOAD_SECOND_UV", slot::SECOND_UV),
        ("PAYLOAD_COLORS", slot::COLORS),
        ("PAYLOAD_LOCAL", slot::LOCAL),
        ("PAYLOAD_CONICAL", slot::CONICAL),
        ("PAYLOAD_CONICAL_FLAGS", slot::CONICAL_FLAGS),
        ("PAYLOAD_COLOR_MATRIX", slot::COLOR_MATRIX),
        (
            "MAX_KERNEL_SAMPLES",
            crate::planner::gaussian::MAX_KERNEL_SAMPLES,
        ),
    ] {
        line(name, u32_value(value));
    }
    for (name, spread) in [
        ("SPREAD_REPEAT", SpreadMode::Repeat),
        ("SPREAD_REFLECT", SpreadMode::Reflect),
    ] {
        line(name, format!("{}u", spread_id(spread) as u32));
    }
    for (name, value) in [
        ("CONICAL_CONCENTRIC", conical::CONCENTRIC),
        ("CONICAL_EMPTY", conical::EMPTY),
        ("CONICAL_STRIP", conical::STRIP),
        ("GAMMA_LINEAR_TO_SRGB", gamma::LINEAR_TO_SRGB),
        ("MASK_LUMINANCE", mask_kind_id(MaskKind::Luminance)),
    ] {
        line(name, format!("{value:?}"));
    }
    for (name, style) in [
        ("STYLE_SOLID", BlurStyle::Solid),
        ("STYLE_INNER", BlurStyle::Inner),
        ("STYLE_OUTER", BlurStyle::Outer),
    ] {
        line(name, format!("{}u", blur_style_id(style) as u32));
    }
    for blend in PipelineBlend::ALL {
        line(
            &format!("BLEND_{}", blend.wgsl_name()),
            format!("{}u", blend.id()),
        );
    }
    for blend in AdvancedBlend::ALL {
        line(
            &format!("ADVANCED_{}", blend.wgsl_name()),
            format!("{}u", blend.id()),
        );
    }
    line("ADVANCED_FILTER_BASE", format!("{ADVANCED_FILTER_BASE}u"));
    prelude
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The record's payload fills the rest of the 512-byte block.
    #[test]
    fn the_payload_fills_the_record() {
        assert_eq!(PAYLOAD_SLOTS, 27);
    }

    /// Every id the WGSL switches on is declared once, by name.
    #[test]
    fn the_prelude_names_every_id() {
        let prelude = wgsl_prelude();
        assert!(prelude.contains("const PAYLOAD_COLOR_MATRIX = 17u;"));
        assert!(prelude.contains("const BLEND_SCREEN = 14u;"));
        assert!(prelude.contains("const ADVANCED_LUMINOSITY = 13u;"));
        assert!(prelude.contains("const STYLE_OUTER = 3u;"));
        assert!(prelude.contains("const SPREAD_REFLECT = 2u;"));
        assert!(prelude.contains("const CONICAL_STRIP = 3.0;"));
    }
}
