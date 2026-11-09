//! valo's integration tests, one module per subject, in one binary so that the
//! crate and its dependencies are linked once rather than once per file, as
//! wasmtime's `tests/all` and Cargo's `tests/testsuite` are. A new subject is a
//! module here, not a file beside `all/`: Cargo builds each of those as a
//! binary of its own.
//!
//! The GPU tests open their own headless device each, and skip (with a note)
//! when the machine has no adapter. Run one subject with its module path as
//! the filter: `cargo test -p valo --test all golden::`.

use std::path::Path;

use valo::Context;

mod backdrop_background;
mod blur_placement;
mod blur_tile_modes;
mod bounded_blur;
mod drawn_over_targets;
mod golden;
mod gradient_endpoints;
mod image_context;
mod layer_blur_edges;
mod mask_blur_styles;
mod perf_probe;
mod persistent_canvas;
mod planner_scenes;
mod plus_darker;
mod raster_cache;
mod recorded_bounds;
mod resources;
mod runtime_filters;
mod runtime_shaders;
mod shared_device;
mod split_targets;
mod text_group_alpha;
mod text_paints;

/// `goldens_dir` is the directory of the checked-in golden PNGs that
/// `valo_harness::assert_golden` compares against, and writes when
/// `VALO_BLESS=1` is set.
fn goldens_dir() -> &'static Path {
    Path::new(concat!(env!("CARGO_MANIFEST_DIR"), "/tests/goldens"))
}

/// `context` opens a [`Context`] on a headless device of its own, or prints
/// that `test` is skipped and returns `None` when the machine has no adapter.
fn context(test: &str) -> Option<Context> {
    let Some((device, queue)) = valo_harness::headless_device() else {
        eprintln!("SKIP {test}: no GPU adapter");
        return None;
    };
    Some(Context::new(device, queue))
}

/// `floats` is `values` as the bytes of a runtime effect's uniforms, in the
/// machine's byte order.
fn floats(values: &[f32]) -> Vec<u8> {
    values
        .iter()
        .flat_map(|value| value.to_ne_bytes())
        .collect()
}
