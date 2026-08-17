//! Path handles: mutable construction (the Flutter/Skia path model —
//! embedders build, draw, `reset`, rebuild) over valo's immutable built
//! paths. The built `Arc<Path>` is cached and invalidated on mutation, so
//! drawing the same path every frame builds it once.

use std::sync::Arc;

use valo::{Path, PathBuilder};

use crate::{
    borrow_mut, dispose_handle, fill_rule, into_handle, ValoCornerRadii, ValoPoint, ValoRect,
};

/// `ValoPath` is a mutable path handle for C embedders.
///
/// Create it with [`valo_path_new`], add verbs, then clip or draw it. Call
/// [`valo_path_reset`] to rebuild in place. Dispose with [`valo_path_dispose`].
/// Handles are not thread-safe.
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

/// `valo_path_new` creates an empty path.
#[no_mangle]
pub extern "C" fn valo_path_new() -> *mut ValoPath {
    into_handle(ValoPath {
        builder: PathBuilder::new(),
        built: None,
    })
}

/// `valo_path_dispose` releases a path handle. Null is a no-op.
///
/// # Safety
/// `path` must be a live [`valo_path_new`] handle (or null).
#[no_mangle]
pub unsafe extern "C" fn valo_path_dispose(path: *mut ValoPath) {
    unsafe { dispose_handle(path) }
}

/// Mutates a path as a `valo_path_*` C function.
///
/// Invocation rustdoc covers that verb's arguments and units. This expansion
/// always appends the shared null-safety contract: a null `path` is a no-op.
macro_rules! path_op {
    ($(#[$doc:meta])* $name:ident($($arg:ident: $ty:ty),*), |$b:ident| $body:expr) => {
        $(#[$doc])*
        ///
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

path_op!(
    /// `valo_path_move_to` starts a new contour at (`x`, `y`).
    valo_path_move_to(x: f32, y: f32),
    |b| b.move_to((x, y))
);
path_op!(
    /// `valo_path_line_to` adds a straight segment to (`x`, `y`).
    valo_path_line_to(x: f32, y: f32),
    |b| b.line_to((x, y))
);
path_op!(
    /// `valo_path_quadratic_to` adds a quadratic Bézier through a control point to (`x`, `y`).
    valo_path_quadratic_to(control_x: f32, control_y: f32, x: f32, y: f32),
    |b| b.quad_to((control_x, control_y), (x, y))
);
path_op!(
    /// `valo_path_cubic_to` adds a cubic Bézier through two control points to (`x`, `y`).
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
path_op!(
    /// `valo_path_close` adds a segment back to the current contour's start.
    ///
    /// It has no effect when no contour is open.
    valo_path_close(),
    |b| b.close()
);
path_op!(
    /// `valo_path_add_rect` adds a closed rectangular contour.
    valo_path_add_rect(rect: ValoRect),
    |b| b.rect(rect.into())
);
path_op!(
    /// `valo_path_add_rounded_rect` adds a closed rounded-rectangle contour.
    valo_path_add_rounded_rect(rect: ValoRect, radii: ValoCornerRadii),
    |b| b.rrect_radii_elliptical(rect, radii.to_elliptical())
);
path_op!(
    /// `valo_path_add_circle` adds a closed circular contour.
    valo_path_add_circle(center: ValoPoint, radius: f32),
    |b| b.circle((center.x, center.y), radius)
);
path_op!(
    /// `valo_path_add_arc` adds a circular arc.
    ///
    /// Angles are radians from the +x axis; a positive sweep turns toward +y,
    /// which is clockwise on screen. An open contour is joined to the arc's
    /// first point by a line, a closed one starts there.
    valo_path_add_arc(center: ValoPoint, radius: f32, start_angle: f32, sweep_angle: f32),
    |b| b.arc((center.x, center.y), radius, start_angle, sweep_angle)
);
path_op!(
    /// `valo_path_add_ellipse` adds an elliptical arc.
    ///
    /// `x_axis_rotation` and the start/sweep angles are radians; a positive
    /// sweep is clockwise on screen. An open contour is joined to the arc's
    /// first point by a line.
    valo_path_add_ellipse(
        center: ValoPoint,
        radius_x: f32,
        radius_y: f32,
        x_axis_rotation: f32,
        start_angle: f32,
        sweep_angle: f32
    ),
    |b| b.ellipse(
        (center.x, center.y),
        [radius_x, radius_y],
        x_axis_rotation,
        start_angle,
        sweep_angle
    )
);
path_op!(
    /// `valo_path_arc_to` rounds the corner between the current point, `corner`, and `next`.
    ///
    /// The circle of `radius` is tangent to both the segment from the current
    /// point to `corner` and the one from `corner` to `next`, reached by a
    /// line. Degenerate input falls back to a line to `corner`.
    valo_path_arc_to(corner: ValoPoint, next: ValoPoint, radius: f32),
    |b| b.arc_to((corner.x, corner.y), (next.x, next.y), radius)
);
path_op!(
    /// `valo_path_reset` clears the path so it can be rebuilt.
    valo_path_reset(),
    |b| *b = PathBuilder::new()
);

/// `valo_path_contains` reports whether `point` lies inside the filled path.
///
/// `rule`: 0 non-zero, 1 even-odd. A null path contains nothing.
///
/// # Safety
/// `path` must be a live handle (or null, which contains nothing).
#[no_mangle]
pub unsafe extern "C" fn valo_path_contains(
    path: *mut ValoPath,
    point: ValoPoint,
    rule: i32,
) -> bool {
    match unsafe { borrow_mut(path) } {
        Some(p) => p
            .built()
            .contains(valo::Point::new(point.x, point.y), fill_rule(rule)),
        None => false,
    }
}
