//! Integer 2D points in scaled millimeters.

use std::ops::{Add, Sub};

/// Scale from millimeters to integer clipper units (Slic3r `SCALING_FACTOR`).
pub const SCALING_FACTOR: i64 = 1_000_000;
pub const SCALING_FACTOR_F64: f64 = 1_000_000.0;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Default)]
pub struct Point {
    pub x: i64,
    pub y: i64,
}

impl Point {
    pub const fn new(x: i64, y: i64) -> Self {
        Self { x, y }
    }

    pub fn from_mm(x: f64, y: f64) -> Self {
        Self {
            x: scale(x),
            y: scale(y),
        }
    }

    pub fn to_mm(self) -> (f64, f64) {
        (unscale(self.x), unscale(self.y))
    }

    pub fn distance_mm(self, other: Self) -> f64 {
        let dx = unscale(self.x - other.x);
        let dy = unscale(self.y - other.y);
        (dx * dx + dy * dy).sqrt()
    }
}

/// C++ `Polyline::clip_end`: drop `max_len_mm` from the tail.
pub fn clip_end(path: &[Point], max_len_mm: f64) -> Vec<Point> {
    const EPS_MM: f64 = 1e-4;
    if path.len() < 2 || max_len_mm <= EPS_MM {
        return path.to_vec();
    }
    let mut pts = path.to_vec();
    let mut remaining = max_len_mm;
    while remaining > EPS_MM && pts.len() >= 2 {
        let last = pts.pop().expect("len >= 2");
        let prev = *pts.last().expect("len >= 1");
        let d = prev.distance_mm(last);
        if d <= remaining {
            remaining -= d;
            continue;
        }
        let t = remaining / d;
        let (px, py) = prev.to_mm();
        let (lx, ly) = last.to_mm();
        pts.push(Point::from_mm(lx + (px - lx) * t, ly + (py - ly) * t));
        break;
    }
    pts
}

impl Add for Point {
    type Output = Self;
    fn add(self, rhs: Self) -> Self {
        Self::new(self.x + rhs.x, self.y + rhs.y)
    }
}

impl Sub for Point {
    type Output = Self;
    fn sub(self, rhs: Self) -> Self {
        Self::new(self.x - rhs.x, self.y - rhs.y)
    }
}

pub fn scale(mm: f64) -> i64 {
    (mm * SCALING_FACTOR_F64).round() as i64
}

pub fn unscale(v: i64) -> f64 {
    v as f64 / SCALING_FACTOR_F64
}

/// Closed or open ring of scaled points. Callers treat the last→first edge as
/// closed for polygons.
pub type Polygon = Vec<Point>;
pub type Polyline = Vec<Point>;

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn clip_end_shortens_closed_square() {
        let path = vec![
            Point::from_mm(0.0, 0.0),
            Point::from_mm(10.0, 0.0),
            Point::from_mm(10.0, 10.0),
            Point::from_mm(0.0, 10.0),
            Point::from_mm(0.0, 0.0),
        ];
        let clipped = clip_end(&path, 0.06);
        assert_eq!(clipped.len(), 5);
        let (x, y) = clipped.last().unwrap().to_mm();
        assert!(x.abs() < 1e-6, "{x}");
        assert!((y - 0.06).abs() < 1e-6, "{y}");
    }
}
