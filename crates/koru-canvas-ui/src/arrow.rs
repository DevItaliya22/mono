// SPDX-License-Identifier: MPL-2.0

//! **An arrow's geometry**: a straight segment from the edge of one object
//! to the edge of the other, along the line between their centres.
//!
//! Drawing and hit testing both take the segment from [`segment`], so an
//! arrow is pressable exactly where it is drawn.

use vello::kurbo::{BezPath, Point, Rect, Vec2};

#[cfg(test)]
mod tests;

/// Where the ray from `rect`'s centre towards `toward` leaves `rect`.
fn exit(rect: Rect, toward: Point) -> Point {
    let c = rect.center();
    let d = toward - c;
    let (hw, hh) = (rect.width() * 0.5, rect.height() * 0.5);
    let tx = if d.x.abs() > f64::EPSILON {
        hw / d.x.abs()
    } else {
        f64::INFINITY
    };
    let ty = if d.y.abs() > f64::EPSILON {
        hh / d.y.abs()
    } else {
        f64::INFINITY
    };
    let t = tx.min(ty).min(1.0);
    c + d * t
}

/// The visible segment of an arrow from `from` to `to`, or `None` when the
/// two overlap and there is nothing between them to draw.
pub(crate) fn segment(from: Rect, to: Rect) -> Option<(Point, Point)> {
    if from.intersect(to).area() > 0.0 || from.center() == to.center() {
        return None;
    }
    let a = exit(from, to.center());
    let b = exit(to, from.center());
    Some((a, b))
}

/// The distance from `p` to the segment `a`–`b`.
pub(crate) fn distance(p: Point, a: Point, b: Point) -> f64 {
    let ab = b - a;
    let len2 = ab.hypot2();
    if len2 <= f64::EPSILON {
        return (p - a).hypot();
    }
    let t = ((p - a).dot(ab) / len2).clamp(0.0, 1.0);
    (p - (a + ab * t)).hypot()
}

/// A filled arrowhead with its tip at `tip`, pointing along `from → tip`.
pub(crate) fn head(from: Point, tip: Point, length: f64, half_width: f64) -> BezPath {
    let d = tip - from;
    let len = d.hypot();
    let dir = if len > f64::EPSILON {
        d / len
    } else {
        Vec2::new(1.0, 0.0)
    };
    let normal = Vec2::new(-dir.y, dir.x);
    let base = tip - dir * length;
    let mut path = BezPath::new();
    path.move_to(tip);
    path.line_to(base + normal * half_width);
    path.line_to(base - normal * half_width);
    path.close_path();
    path
}
