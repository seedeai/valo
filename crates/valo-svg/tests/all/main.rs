//! valo-svg's integration tests, one module per subject, in one binary so that
//! the crate and its dependencies are linked once rather than once per file. A
//! new subject is a module here, not a file beside `all/`: Cargo builds each of
//! those as a binary of its own.

mod export;
mod translate;
