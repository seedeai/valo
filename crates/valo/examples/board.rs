//! Interactive Figma-style board (the stress scene, ~3.3k draws):
//! drag = pan, scroll/pinch = zoom about the cursor, `1` = 100%, `0` = fit,
//! Esc quits. The HUD strip is rendered by valo itself.
//!
//!   cargo run -p valo --example board --release

use std::sync::Arc;

fn main() {
    let fonts = valo_harness::example_fonts();
    let board = Arc::new(valo_harness::scenes::figma_board(&fonts));
    valo_harness::interactive::run_pan_zoom("valo — board", fonts, board);
}
