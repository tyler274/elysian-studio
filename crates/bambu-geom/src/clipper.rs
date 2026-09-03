//! Clipper2 (pure Rust) boolean and offset helpers.

use clipper2_rust::clipper::{difference_64, inflate_paths_64, intersect_64, union_subjects_64};
use clipper2_rust::core::{FillRule, Path64, Paths64, Point64};
use clipper2_rust::offset::{EndType, JoinType};

use crate::point::{Point, Polygon, SCALING_FACTOR_F64};

fn to_path64(poly: &[Point]) -> Path64 {
    poly.iter().map(|p| Point64::new(p.x, p.y)).collect()
}

fn from_path64(path: &Path64) -> Polygon {
    path.iter().map(|p| Point::new(p.x, p.y)).collect()
}

fn to_paths64(polygons: &[Polygon]) -> Paths64 {
    polygons.iter().map(|p| to_path64(p)).collect()
}

fn from_paths64(paths: &Paths64) -> Vec<Polygon> {
    paths
        .iter()
        .map(from_path64)
        .filter(|p| p.len() >= 3)
        .collect()
}

/// Union a set of polygons (NonZero fill).
pub fn union_polygons(polygons: &[Polygon]) -> Vec<Polygon> {
    if polygons.is_empty() {
        return Vec::new();
    }
    let subjects = to_paths64(polygons);
    from_paths64(&union_subjects_64(&subjects, FillRule::NonZero))
}

/// Subjects minus clips (NonZero fill).
pub fn difference_polygons(subjects: &[Polygon], clips: &[Polygon]) -> Vec<Polygon> {
    if subjects.is_empty() {
        return Vec::new();
    }
    if clips.is_empty() {
        return union_polygons(subjects);
    }
    from_paths64(&difference_64(
        &to_paths64(subjects),
        &to_paths64(clips),
        FillRule::NonZero,
    ))
}

/// Intersection of two polygon sets (NonZero fill).
pub fn intersect_polygons(a: &[Polygon], b: &[Polygon]) -> Vec<Polygon> {
    if a.is_empty() || b.is_empty() {
        return Vec::new();
    }
    from_paths64(&intersect_64(
        &to_paths64(a),
        &to_paths64(b),
        FillRule::NonZero,
    ))
}

/// Offset polygons by `delta_mm` (positive = expand, negative = shrink).
pub fn offset_polygons(polygons: &[Polygon], delta_mm: f64) -> Vec<Polygon> {
    if polygons.is_empty() {
        return Vec::new();
    }
    let paths: Paths64 = polygons.iter().map(|p| to_path64(p)).collect();
    let delta = delta_mm * SCALING_FACTOR_F64;
    inflate_paths_64(&paths, delta, JoinType::Miter, EndType::Polygon, 2.0, 0.25)
        .iter()
        .map(from_path64)
        .filter(|p| p.len() >= 3)
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::point::Point;

    fn square(size_mm: f64) -> Polygon {
        vec![
            Point::from_mm(0.0, 0.0),
            Point::from_mm(size_mm, 0.0),
            Point::from_mm(size_mm, size_mm),
            Point::from_mm(0.0, size_mm),
        ]
    }

    #[test]
    fn union_overlapping_squares() {
        let a = square(10.0);
        let b = vec![
            Point::from_mm(5.0, 5.0),
            Point::from_mm(15.0, 5.0),
            Point::from_mm(15.0, 15.0),
            Point::from_mm(5.0, 15.0),
        ];
        let out = union_polygons(&[a, b]);
        assert_eq!(out.len(), 1);
        assert!(out[0].len() >= 6);
    }

    #[test]
    fn offset_expands_square() {
        let poly = square(10.0);
        let grown = offset_polygons(&[poly], 1.0);
        assert_eq!(grown.len(), 1);
        let xs: Vec<i64> = grown[0].iter().map(|p| p.x).collect();
        let min_x = *xs.iter().min().unwrap();
        let max_x = *xs.iter().max().unwrap();
        // 10mm square grown by 1mm → width ~12mm
        let width_mm = crate::unscale(max_x - min_x);
        assert!((width_mm - 12.0).abs() < 0.05, "width_mm={width_mm}");
    }

    #[test]
    fn difference_removes_overlap() {
        let a = square(10.0);
        let hole = vec![
            Point::from_mm(2.0, 2.0),
            Point::from_mm(8.0, 2.0),
            Point::from_mm(8.0, 8.0),
            Point::from_mm(2.0, 8.0),
        ];
        let out = difference_polygons(&[a.clone()], &[hole]);
        assert!(!out.is_empty());
        let overlap = intersect_polygons(&[a], &[square(1.0)]);
        assert!(!overlap.is_empty());
    }
}
