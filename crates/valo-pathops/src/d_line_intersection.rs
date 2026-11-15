//! Port of Skia's src/pathops/SkDLineIntersection.cpp: where a line meets a line, or a
//! horizontal or vertical line.

use crate::intersections::SkIntersections;
use crate::path_ops_line::SkDLine;
use crate::path_ops_point::SkDPoint;
use crate::path_ops_types::{
    almost_equal_ulps, approximately_equal, approximately_zero, between, not_almost_dequal_ulps,
    not_almost_equal_ulps_pin, precisely_between, sk_pin_t, zero_or_one,
};

impl SkIntersections {
    pub(crate) fn clean_up_parallel_lines(&mut self, parallel: bool) {
        while self.used > 2 {
            self.remove_one(1);
        }
        if self.used == 2 && !parallel {
            let start_match = self.t[0][0] == 0.0 || zero_or_one(self.t[1][0]);
            let end_match = self.t[0][1] == 1.0 || zero_or_one(self.t[1][1]);
            if (!start_match && !end_match) || approximately_equal(self.t[0][0], self.t[0][1]) {
                debug_assert!(start_match || end_match);
                if start_match
                    && end_match
                    && (self.t[0][0] != 0.0 || !zero_or_one(self.t[1][0]))
                    && self.t[0][1] == 1.0
                    && zero_or_one(self.t[1][1])
                {
                    self.remove_one(0);
                } else {
                    self.remove_one(i32::from(end_match));
                }
            }
        }
        if self.used == 2 {
            self.is_coincident[0] = 0x03;
            self.is_coincident[1] = 0x03;
        }
    }

    pub(crate) fn compute_points(&mut self, line: &SkDLine, used: i32) {
        self.pt[0] = line.pt_at_t(self.t[0][0]);
        self.used = used as u8;
        if self.used == 2 {
            self.pt[1] = line.pt_at_t(self.t[0][1]);
        }
    }

    /// `intersectRay(const SkDLine& a, const SkDLine& b)`.
    pub(crate) fn intersect_ray_line(&mut self, a: &SkDLine, b: &SkDLine) -> i32 {
        self.max = 2;
        let a_len = a[1] - a[0];
        let b_len = b[1] - b[0];
        /* Slopes match when denom goes to zero:
                          axLen / ayLen ==                   bxLen / byLen
        (ayLen * byLen) * axLen / ayLen == (ayLen * byLen) * bxLen / byLen
                 byLen  * axLen         ==  ayLen          * bxLen
                 byLen  * axLen         -   ayLen          * bxLen == 0 ( == denom )
         */
        let denom = b_len.y * a_len.x - a_len.y * b_len.x;
        let used;
        if !approximately_zero(denom) {
            let ab0 = a[0] - b[0];
            let mut numer_a = ab0.y * b_len.x - b_len.y * ab0.x;
            let mut numer_b = ab0.y * a_len.x - a_len.y * ab0.x;
            numer_a /= denom;
            numer_b /= denom;
            self.t[0][0] = numer_a;
            self.t[1][0] = numer_b;
            used = 1;
        } else {
            /* See if the axis intercepts match:
                      ay - ax * ayLen / axLen  ==          by - bx * ayLen / axLen
             axLen * (ay - ax * ayLen / axLen) == axLen * (by - bx * ayLen / axLen)
             axLen *  ay - ax * ayLen          == axLen *  by - bx * ayLen
            */
            if !almost_equal_ulps(
                a_len.x * a[0].y - a_len.y * a[0].x,
                a_len.x * b[0].y - a_len.y * b[0].x,
            ) {
                self.used = 0;
                return 0;
            }
            // there's no great answer for intersection points for coincident rays, but return something
            self.t[1][0] = 0.0;
            self.t[0][0] = 0.0;
            self.t[1][1] = 1.0;
            self.t[1][0] = 1.0;
            used = 2;
        }
        self.compute_points(a, used);
        self.used as i32
    }

    /// `intersect(const SkDLine& a, const SkDLine& b)`.
    // note that this only works if both lines are neither horizontal nor vertical
    pub(crate) fn intersect_line_line(&mut self, a: &SkDLine, b: &SkDLine) -> i32 {
        self.max = 3; // note that we clean up so that there is no more than two in the end
                      // see if end points intersect the opposite line
        let mut t;
        for i_a in 0..2 {
            t = b.exact_point(&a[i_a]);
            if t >= 0.0 {
                self.insert(i_a as f64, t, &a[i_a]);
            }
        }
        for i_b in 0..2 {
            t = a.exact_point(&b[i_b]);
            if t >= 0.0 {
                self.insert(t, i_b as f64, &b[i_b]);
            }
        }
        /* Determine the intersection point of two line segments
        Return FALSE if the lines don't intersect
        from: http://paulbourke.net/geometry/lineline2d/ */
        let ax_len = a[1].x - a[0].x;
        let ay_len = a[1].y - a[0].y;
        let bx_len = b[1].x - b[0].x;
        let by_len = b[1].y - b[0].y;
        /* Slopes match when denom goes to zero:
                          axLen / ayLen ==                   bxLen / byLen
        (ayLen * byLen) * axLen / ayLen == (ayLen * byLen) * bxLen / byLen
                 byLen  * axLen         ==  ayLen          * bxLen
                 byLen  * axLen         -   ayLen          * bxLen == 0 ( == denom )
         */
        let ax_by_len = ax_len * by_len;
        let ay_bx_len = ay_len * bx_len;
        // detect parallel lines the same way here and in SkOpAngle operator <
        // so that non-parallel means they are also sortable
        let unparallel = if self.allow_near {
            not_almost_equal_ulps_pin(ax_by_len, ay_bx_len)
        } else {
            not_almost_dequal_ulps(ax_by_len, ay_bx_len)
        };
        if unparallel && self.used == 0 {
            let ab0y = a[0].y - b[0].y;
            let ab0x = a[0].x - b[0].x;
            let numer_a = ab0y * bx_len - by_len * ab0x;
            let numer_b = ab0y * ax_len - ay_len * ab0x;
            let denom = ax_by_len - ay_bx_len;
            if between(0.0, numer_a, denom) && between(0.0, numer_b, denom) {
                self.t[0][0] = numer_a / denom;
                self.t[1][0] = numer_b / denom;
                self.compute_points(a, 1);
            }
        }
        /* Allow tracking that both sets of end points are near each other -- the lines are entirely
          coincident -- even when the end points are not exactly the same.
          Mark this as a 'wild card' for the end points, so that either point is considered totally
          coincident. Then, avoid folding the lines over each other, but allow either end to mate
          to the next set of lines.
        */
        if self.allow_near || !unparallel {
            let mut a_near_b = [0.0f64; 2];
            let mut b_near_a = [0.0f64; 2];
            let mut a_not_b = [false, false];
            let mut b_not_a = [false, false];
            let mut near_count = 0;
            for index in 0..2 {
                t = b.near_point(&a[index], Some(&mut a_not_b[index]));
                a_near_b[index] = t;
                near_count += i32::from(t >= 0.0);
                t = a.near_point(&b[index], Some(&mut b_not_a[index]));
                b_near_a[index] = t;
                near_count += i32::from(t >= 0.0);
            }
            if near_count > 0 {
                // Skip if each segment contributes to one end point.
                if near_count != 2 || a_not_b[0] == a_not_b[1] {
                    for i_a in 0..2 {
                        if !a_not_b[i_a] {
                            continue;
                        }
                        let nearer = usize::from(a_near_b[i_a] > 0.5);
                        if !b_not_a[nearer] {
                            continue;
                        }
                        debug_assert!(a[i_a] != b[nearer]);
                        self.insert_near(i_a as f64, nearer as f64, &a[i_a], &b[nearer]);
                        a_near_b[i_a] = -1.0;
                        b_near_a[nearer] = -1.0;
                        near_count -= 2;
                    }
                }
                if near_count > 0 {
                    for i_a in 0..2 {
                        if a_near_b[i_a] >= 0.0 {
                            self.insert(i_a as f64, a_near_b[i_a], &a[i_a]);
                        }
                    }
                    for i_b in 0..2 {
                        if b_near_a[i_b] >= 0.0 {
                            self.insert(b_near_a[i_b], i_b as f64, &b[i_b]);
                        }
                    }
                }
            }
        }
        self.clean_up_parallel_lines(!unparallel);
        debug_assert!(self.used <= 2);
        self.used as i32
    }
}

fn horizontal_coincident(line: &SkDLine, y: f64) -> i32 {
    let mut min = line[0].y;
    let mut max = line[1].y;
    if min > max {
        std::mem::swap(&mut min, &mut max);
    }
    if min > y || max < y {
        return 0;
    }
    if almost_equal_ulps(min, max) && max - min < (line[0].x - line[1].x).abs() {
        return 2;
    }
    1
}

impl SkIntersections {
    /// `static HorizontalIntercept(const SkDLine& line, double y)`.
    pub(crate) fn horizontal_intercept_line(line: &SkDLine, y: f64) -> f64 {
        debug_assert!(line[1].y != line[0].y);
        sk_pin_t((y - line[0].y) / (line[1].y - line[0].y))
    }

    /// `horizontal(const SkDLine&, double left, double right, double y, bool flipped)`.
    pub(crate) fn horizontal_line(
        &mut self,
        line: &SkDLine,
        left: f64,
        right: f64,
        y: f64,
        flipped: bool,
    ) -> i32 {
        self.max = 3; // clean up parallel at the end will limit the result to 2 at the most
                      // see if end points intersect the opposite line
        let mut t;
        let left_pt = SkDPoint { x: left, y };
        t = line.exact_point(&left_pt);
        if t >= 0.0 {
            self.insert(t, f64::from(u8::from(flipped)), &left_pt);
        }
        if left != right {
            let right_pt = SkDPoint { x: right, y };
            t = line.exact_point(&right_pt);
            if t >= 0.0 {
                self.insert(t, f64::from(u8::from(!flipped)), &right_pt);
            }
            for index in 0..2 {
                t = SkDLine::exact_point_h(&line[index], left, right, y);
                if t >= 0.0 {
                    self.insert(
                        index as f64,
                        if flipped { 1.0 - t } else { t },
                        &line[index],
                    );
                }
            }
        }
        let result = horizontal_coincident(line, y);
        if result == 1 && self.used == 0 {
            self.t[0][0] = SkIntersections::horizontal_intercept_line(line, y);
            let x_intercept = line[0].x + self.t[0][0] * (line[1].x - line[0].x);
            if between(left, x_intercept, right) {
                self.t[1][0] = (x_intercept - left) / (right - left);
                if flipped {
                    // OPTIMIZATION: ? instead of swapping, pass original line, use [1].fX - [0].fX
                    for index in 0..result as usize {
                        self.t[1][index] = 1.0 - self.t[1][index];
                    }
                }
                self.pt[0].x = x_intercept;
                self.pt[0].y = y;
                self.used = 1;
            }
        }
        if self.allow_near || result == 2 {
            t = line.near_point(&left_pt, None);
            if t >= 0.0 {
                self.insert(t, f64::from(u8::from(flipped)), &left_pt);
            }
            if left != right {
                let right_pt = SkDPoint { x: right, y };
                t = line.near_point(&right_pt, None);
                if t >= 0.0 {
                    self.insert(t, f64::from(u8::from(!flipped)), &right_pt);
                }
                for index in 0..2 {
                    t = SkDLine::near_point_h(&line[index], left, right, y);
                    if t >= 0.0 {
                        self.insert(
                            index as f64,
                            if flipped { 1.0 - t } else { t },
                            &line[index],
                        );
                    }
                }
            }
        }
        self.clean_up_parallel_lines(result == 2);
        self.used as i32
    }
}

fn vertical_coincident(line: &SkDLine, x: f64) -> i32 {
    let mut min = line[0].x;
    let mut max = line[1].x;
    if min > max {
        std::mem::swap(&mut min, &mut max);
    }
    if !precisely_between(min, x, max) {
        return 0;
    }
    if almost_equal_ulps(min, max) {
        return 2;
    }
    1
}

impl SkIntersections {
    /// `static VerticalIntercept(const SkDLine& line, double x)`.
    pub(crate) fn vertical_intercept_line(line: &SkDLine, x: f64) -> f64 {
        debug_assert!(line[1].x != line[0].x);
        sk_pin_t((x - line[0].x) / (line[1].x - line[0].x))
    }

    /// `vertical(const SkDLine&, double top, double bottom, double x, bool flipped)`.
    pub(crate) fn vertical_line(
        &mut self,
        line: &SkDLine,
        top: f64,
        bottom: f64,
        x: f64,
        flipped: bool,
    ) -> i32 {
        self.max = 3; // cleanup parallel lines will bring this back line
                      // see if end points intersect the opposite line
        let mut t;
        let top_pt = SkDPoint { x, y: top };
        t = line.exact_point(&top_pt);
        if t >= 0.0 {
            self.insert(t, f64::from(u8::from(flipped)), &top_pt);
        }
        if top != bottom {
            let bottom_pt = SkDPoint { x, y: bottom };
            t = line.exact_point(&bottom_pt);
            if t >= 0.0 {
                self.insert(t, f64::from(u8::from(!flipped)), &bottom_pt);
            }
            for index in 0..2 {
                t = SkDLine::exact_point_v(&line[index], top, bottom, x);
                if t >= 0.0 {
                    self.insert(
                        index as f64,
                        if flipped { 1.0 - t } else { t },
                        &line[index],
                    );
                }
            }
        }
        let result = vertical_coincident(line, x);
        if result == 1 && self.used == 0 {
            self.t[0][0] = SkIntersections::vertical_intercept_line(line, x);
            let y_intercept = line[0].y + self.t[0][0] * (line[1].y - line[0].y);
            if between(top, y_intercept, bottom) {
                self.t[1][0] = (y_intercept - top) / (bottom - top);
                if flipped {
                    // OPTIMIZATION: instead of swapping, pass original line, use [1].fY - [0].fY
                    for index in 0..result as usize {
                        self.t[1][index] = 1.0 - self.t[1][index];
                    }
                }
                self.pt[0].x = x;
                self.pt[0].y = y_intercept;
                self.used = 1;
            }
        }
        if self.allow_near || result == 2 {
            t = line.near_point(&top_pt, None);
            if t >= 0.0 {
                self.insert(t, f64::from(u8::from(flipped)), &top_pt);
            }
            if top != bottom {
                let bottom_pt = SkDPoint { x, y: bottom };
                t = line.near_point(&bottom_pt, None);
                if t >= 0.0 {
                    self.insert(t, f64::from(u8::from(!flipped)), &bottom_pt);
                }
                for index in 0..2 {
                    t = SkDLine::near_point_v(&line[index], top, bottom, x);
                    if t >= 0.0 {
                        self.insert(
                            index as f64,
                            if flipped { 1.0 - t } else { t },
                            &line[index],
                        );
                    }
                }
            }
        }
        self.clean_up_parallel_lines(result == 2);
        debug_assert!(self.used <= 2);
        self.used as i32
    }
}
