//! Shared bench setup — one fonts source of truth lives in the harness.

use std::sync::Arc;
use valo::FontCollection;

pub fn fonts() -> Arc<FontCollection> {
    valo_harness::example_fonts()
}
