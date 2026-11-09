//! valo-codec-software's integration tests, one module per subject, in one
//! binary so that the crate and its dependencies are linked once rather than
//! once per file. A new subject is a module here, not a file beside `all/`:
//! Cargo builds each of those as a binary of its own. Each subject needs the
//! format features it reads or writes.

#[cfg(all(feature = "png", feature = "gif"))]
mod decode;
#[cfg(all(feature = "png", feature = "jpeg", feature = "webp"))]
mod encode;
