//! valo-codec's integration tests, one module per subject, in one binary so
//! that the crate and its dependencies are linked once rather than once per
//! file, and the scripted decoder they share is compiled once. A new subject is
//! a module here, not a file beside `all/`: Cargo builds each of those as a
//! binary of its own.

mod gpu_frames;
mod loader;
mod support;
#[cfg(all(feature = "worker", not(target_arch = "wasm32")))]
mod worker;
