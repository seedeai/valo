//! valo-text's integration tests, one module per subject, in one binary so that
//! the crate and its dependencies are linked once rather than once per file. A
//! new subject is a module here, not a file beside `all/`: Cargo builds each of
//! those as a binary of its own.

mod collection;
mod colr;
mod font_styles;
mod raster;
#[cfg(feature = "woff2")]
mod woff2;

/// `asset` reads the font file `name` from the repository's test fonts.
fn asset(name: &str) -> Vec<u8> {
    let dir = concat!(env!("CARGO_MANIFEST_DIR"), "/../../assets/fonts");
    std::fs::read(format!("{dir}/{name}")).unwrap()
}
