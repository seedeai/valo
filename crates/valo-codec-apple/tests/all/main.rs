//! valo-codec-apple's integration tests, one module per subject, in one binary
//! so that the crate and its dependencies are linked once rather than once per
//! file. A new subject is a module here, not a file beside `all/`: Cargo builds
//! each of those as a binary of its own. ImageIO is Apple's, so elsewhere the
//! binary is empty.
#![cfg(any(target_os = "macos", target_os = "ios"))]

mod lifetime;
mod native;
