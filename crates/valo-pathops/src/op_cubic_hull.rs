//! Port of Skia's src/pathops/SkOpCubicHull.cpp: the convex hull of a cubic's points.

use crate::core::float_bits::std_min;
use crate::path_ops_cubic::{other_two, SkDCubic};
use crate::path_ops_types::{approximately_equal, approximately_zero};

fn rotate(cubic: &SkDCubic, zero: usize, index: usize, rot_path: &mut SkDCubic) -> bool {
    let dy = cubic[index].y - cubic[zero].y;
    let dx = cubic[index].x - cubic[zero].x;
    if approximately_zero(dy) {
        if approximately_zero(dx) {
            return false;
        }
        *rot_path = *cubic;
        if dy != 0.0 {
            rot_path[index].y = cubic[zero].y;
            let mask = other_two(index as i32, zero as i32);
            let side1 = (index as i32 ^ mask) as usize;
            let side2 = (zero as i32 ^ mask) as usize;
            if approximately_equal(cubic[side1].y, cubic[zero].y) {
                rot_path[side1].y = cubic[zero].y;
            }
            if approximately_equal(cubic[side2].y, cubic[zero].y) {
                rot_path[side2].y = cubic[zero].y;
            }
        }
        return true;
    }
    for i in 0..4 {
        rot_path[i].x = cubic[i].x * dx + cubic[i].y * dy;
        rot_path[i].y = cubic[i].y * dx - cubic[i].x * dy;
    }
    true
}

// Returns 0 if negative, 1 if zero, 2 if positive
fn side(x: f64) -> i32 {
    i32::from(x > 0.0) + i32::from(x >= 0.0)
}

impl SkDCubic {
    /* Given a cubic, find the convex hull described by the end and control points.
       The hull may have 3 or 4 points. Cubics that degenerate into a point or line
       are not considered.

       The hull is computed by assuming that three points, if unique and non-linear,
       form a triangle. The fourth point may replace one of the first three, may be
       discarded if in the triangle or on an edge, or may be inserted between any of
       the three to form a convex quadralateral.

       The indices returned in order describe the convex hull.
    */
    /// Declared in SkPathOpsCubic.h. Skia's `char order[4]` receives point indices.
    pub(crate) fn convex_hull(&self, order: &mut [i8; 4]) -> i32 {
        // find top point
        let mut y_min: usize = 0;
        for index in 1..4 {
            if self.pts[y_min].y > self.pts[index].y
                || (self.pts[y_min].y == self.pts[index].y && self.pts[y_min].x > self.pts[index].x)
            {
                y_min = index;
            }
        }
        order[0] = y_min as i8;
        let mut mid_x: i32 = -1;
        let mut backup_y_min: i32 = -1;
        for _pass in 0..2 {
            for index in 0..4_usize {
                if index == y_min {
                    continue;
                }
                // rotate line from (yMin, index) to axis
                // see if remaining two points are both above or below
                // use this to find mid
                let mask = other_two(y_min as i32, index as i32);
                let side1 = (y_min as i32 ^ mask) as usize;
                let side2 = (index as i32 ^ mask) as usize;
                let mut rot_path = SkDCubic::default();
                if !rotate(self, y_min, index, &mut rot_path) {
                    // ! if cbc[yMin]==cbc[idx]
                    order[1] = side1 as i8;
                    order[2] = side2 as i8;
                    return 3;
                }
                let mut sides = side(rot_path[side1].y - rot_path[y_min].y);
                sides ^= side(rot_path[side2].y - rot_path[y_min].y);
                if sides == 2 {
                    // '2' means one remaining point <0, one >0
                    if mid_x >= 0 {
                        // one of the control points is equal to an end point
                        order[0] = 0;
                        order[1] = 3;
                        if self.pts[1] == self.pts[0] || self.pts[1] == self.pts[3] {
                            order[2] = 2;
                            return 3;
                        }
                        if self.pts[2] == self.pts[0] || self.pts[2] == self.pts[3] {
                            order[2] = 1;
                            return 3;
                        }
                        // one of the control points may be very nearly but not exactly equal --
                        let dist1_0 = self.pts[1].distance_squared(&self.pts[0]);
                        let dist1_3 = self.pts[1].distance_squared(&self.pts[3]);
                        let dist2_0 = self.pts[2].distance_squared(&self.pts[0]);
                        let dist2_3 = self.pts[2].distance_squared(&self.pts[3]);
                        let smallest1dist_sq = std_min(dist1_0, dist1_3);
                        let smallest2dist_sq = std_min(dist2_0, dist2_3);
                        if approximately_zero(std_min(smallest1dist_sq, smallest2dist_sq)) {
                            order[2] = if smallest1dist_sq < smallest2dist_sq {
                                2
                            } else {
                                1
                            };
                            return 3;
                        }
                    }
                    mid_x = index as i32;
                } else if sides == 0 {
                    // '0' means both to one side or the other
                    backup_y_min = index as i32;
                }
            }
            if mid_x >= 0 {
                break;
            }
            if backup_y_min < 0 {
                break;
            }
            y_min = backup_y_min as usize;
            backup_y_min = -1;
        }
        if mid_x < 0 {
            mid_x = y_min as i32 ^ 3; // choose any other point
        }
        let mask = other_two(y_min as i32, mid_x);
        let least = y_min as i32 ^ mask;
        let most = mid_x ^ mask;
        order[0] = y_min as i8;
        order[1] = least as i8;

        // see if mid value is on same side of line (least, most) as yMin
        let mut mid_path = SkDCubic::default();
        if !rotate(self, least as usize, most as usize, &mut mid_path) {
            // ! if cbc[least]==cbc[most]
            order[2] = mid_x as i8;
            return 3;
        }
        let mut mid_sides = side(mid_path[y_min].y - mid_path[least as usize].y);
        mid_sides ^= side(mid_path[mid_x as usize].y - mid_path[least as usize].y);
        if mid_sides != 2 {
            // if mid point is not between
            order[2] = most as i8;
            return 3; // result is a triangle
        }
        order[2] = mid_x as i8;
        order[3] = most as i8;
        4 // result is a quadralateral
    }
}
