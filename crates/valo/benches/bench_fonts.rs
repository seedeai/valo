//! Shared bench setup — one fonts source of truth lives in the harness.

use valo::FontCollection;

pub fn fonts() -> FontCollection {
    valo_harness::example_fonts()
}
