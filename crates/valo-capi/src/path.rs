//! Path handles: mutable construction (the Flutter/Skia path model —
//! embedders build, draw, `reset`, rebuild) over valo's immutable built
//! paths. The built `Arc<Path>` is cached and invalidated on mutation, so
//! drawing the same path every frame builds it once.

use std::sync::Arc;

use valo::{Path, PathBuilder};

use crate::{borrow_mut, dispose_handle, into_handle, ValoCornerRadii, ValoPoint, ValoRect};

pub struct ValoPath {
    builder: PathBuilder,
    built: Option<Arc<Path>>,
}

impl ValoPath {
    /// The built path, rebuilt only after mutations.
    pub(crate) fn built(&mut self) -> Arc<Path> {
        self.built
            .get_or_insert_with(|| self.builder.clone().build())
            .clone()
    }

    fn mutate(&mut self, change: impl FnOnce(&mut PathBuilder)) {
        change(&mut self.builder);
        self.built = None;
    }
}

#[no_mangle]
pub extern "C" fn valo_path_new() -> *mut ValoPath {
    into_handle(ValoPath {
        builder: PathBuilder::new(),
        built: None,
    })
}

/// # Safety
/// `path` must be a live [`valo_path_new`] handle (or null).
#[no_mangle]
pub unsafe extern "C" fn valo_path_dispose(path: *mut ValoPath) {
    unsafe { dispose_handle(path) }
}

macro_rules! path_op {
    ($(#[$doc:meta])* $name:ident($($arg:ident: $ty:ty),*), |$b:ident| $body:expr) => {
        $(#[$doc])*
        /// # Safety
        /// `path` must be a live handle (or null, a no-op).
        #[no_mangle]
        pub unsafe extern "C" fn $name(path: *mut ValoPath $(, $arg: $ty)*) {
            if let Some(p) = unsafe { borrow_mut(path) } {
                p.mutate(|$b| {
                    $body;
                });
            }
        }
    };
}

path_op!(valo_path_move_to(x: f32, y: f32), |b| b.move_to((x, y)));
path_op!(valo_path_line_to(x: f32, y: f32), |b| b.line_to((x, y)));
path_op!(
    valo_path_quadratic_to(control_x: f32, control_y: f32, x: f32, y: f32),
    |b| b.quad_to((control_x, control_y), (x, y))
);
path_op!(
    valo_path_cubic_to(
        control1_x: f32,
        control1_y: f32,
        control2_x: f32,
        control2_y: f32,
        x: f32,
        y: f32
    ),
    |b| b.cubic_to((control1_x, control1_y), (control2_x, control2_y), (x, y))
);
path_op!(valo_path_close(), |b| b.close());
path_op!(valo_path_add_rect(rect: ValoRect), |b| b.rect(rect.into()));
path_op!(valo_path_add_rounded_rect(rect: ValoRect, radii: ValoCornerRadii), |b| b
    .rrect_radii_elliptical(rect.into(), radii.to_elliptical()));
path_op!(valo_path_add_circle(center: ValoPoint, radius: f32), |b| b
    .circle((center.x, center.y), radius));
path_op!(valo_path_reset(), |b| *b = PathBuilder::new());
