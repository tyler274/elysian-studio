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
