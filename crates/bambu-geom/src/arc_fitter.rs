//! C++ `Circle.cpp` / `ArcFitter.cpp` / `MultiPoint::_douglas_peucker`.
//!
//! Fits G2/G3 arcs to polylines and Douglas-Peucker-simplifies the leftovers.
//! Coordinates are Slic3r scaled integers; circle math is done in millimeters.

use crate::point::{Point, SCALING_FACTOR_F64};

/// C++ `EPSILON` (1e-4 mm).
const EPSILON_MM: f64 = 1e-4;
/// C++ `Circle::ZERO_TOLERANCE`.
const ZERO_TOLERANCE: f64 = 0.000005;
/// C++ `Parallel_area_threshold` compared in mm² (area scales twice).
const PARALLEL_AREA_MM2: f64 = 0.0001;
/// C++ `DEFAULT_SCALED_MAX_RADIUS` = 2000 mm.
const DEFAULT_MAX_RADIUS_MM: f64 = 2000.0;
/// C++ `DEFAULT_ARC_LENGTH_PERCENT_TOLERANCE`.
const PATH_TOLERANCE_PERCENT: f64 = 0.05;
/// Degeneracy of the 3-point circumcircle determinant (C++ `SCALED_EPSILON` vs scaled²).
const CIRCLE_DET_EPS_MM2: f64 = 1e-9;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ArcDir {
    Cw,
    Ccw,
}

#[derive(Debug, Clone, Copy, PartialEq)]
pub struct ArcSegment {
    pub center_mm: (f64, f64),
    pub radius_mm: f64,
    pub start: Point,
    pub end: Point,
    pub length_mm: f64,
    pub dir: ArcDir,
}

#[derive(Debug, Clone, Copy, PartialEq)]
pub enum PathFitKind {
    Linear,
    Arc(ArcSegment),
}

#[derive(Debug, Clone, Copy, PartialEq)]
pub struct PathFit {
    pub start: usize,
    pub end: usize,
    pub kind: PathFitKind,
}

struct Circle {
    center_mm: (f64, f64),
    radius_mm: f64,
}

impl Circle {
    fn polar(&self, p: Point) -> f64 {
        let (x, y) = p.to_mm();
        let mut a = (y - self.center_mm.1).atan2(x - self.center_mm.0);
        if a < 0.0 {
            a += std::f64::consts::TAU;
        }
        a
    }
}

/// C++ `MultiPoint::_douglas_peucker`. `tolerance_mm` is unscaled.
pub fn douglas_peucker(pts: &[Point], tolerance_mm: f64) -> Vec<Point> {
    if pts.is_empty() {
        return Vec::new();
    }
    let tolerance_sq = (tolerance_mm * SCALING_FACTOR_F64).powi(2);
    let mut result = Vec::with_capacity(pts.len());
    result.push(pts[0]);
    if pts.len() == 1 {
        return result;
    }
    let mut anchor_idx = 0;
    let mut floater_idx = pts.len() - 1;
    let mut stack = vec![floater_idx];
    loop {
        let mut max_dist_sq = 0.0;
        let mut furthest = anchor_idx;
        for i in (anchor_idx + 1)..floater_idx {
            let dist_sq = distance_to_squared(pts[i], pts[anchor_idx], pts[floater_idx]);
            if dist_sq > max_dist_sq {
                max_dist_sq = dist_sq;
                furthest = i;
            }
        }
        if max_dist_sq <= tolerance_sq {
            result.push(pts[floater_idx]);
            anchor_idx = floater_idx;
            stack.pop();
            match stack.last().copied() {
                Some(idx) => floater_idx = idx,
                None => break,
            }
        } else {
            floater_idx = furthest;
            stack.push(floater_idx);
        }
    }
    result
}

/// C++ `ArcFitter::do_arc_fitting_and_simplify`.
pub fn fit_arcs_and_simplify(points: &[Point], tolerance_mm: f64) -> (Vec<Point>, Vec<PathFit>) {
    if points.len() < 2 {
        return (points.to_vec(), Vec::new());
    }
    let mut fits = if tolerance_mm > EPSILON_MM {
        do_arc_fitting(points, tolerance_mm)
    } else {
        vec![PathFit {
            start: 0,
            end: points.len() - 1,
            kind: PathFitKind::Linear,
        }]
    };
    if fits.len() == 1 && matches!(fits[0].kind, PathFitKind::Linear) {
        let simplified = douglas_peucker(points, tolerance_mm);
        fits[0].end = simplified.len() - 1;
        return (simplified, fits);
    }
    let mut simplified = Vec::with_capacity(points.len());
    simplified.push(points[0]);
    let mut reduce = vec![0usize; fits.len()];
    for (i, fit) in fits.iter().enumerate() {
        let part = &points[fit.start..=fit.end];
        let slim = douglas_peucker(part, tolerance_mm);
        reduce[i] = part.len().saturating_sub(slim.len());
        simplified.extend(slim.into_iter().skip(1));
    }
    for i in 1..reduce.len() {
        reduce[i] += reduce[i - 1];
    }
    for j in 0..fits.len() {
        fits[j].end -= reduce[j];
        if j + 1 < fits.len() {
            fits[j + 1].start = fits[j].end;
        }
    }
    (simplified, fits)
}

fn do_arc_fitting(points: &[Point], tolerance_mm: f64) -> Vec<PathFit> {
    let mut result = Vec::new();
    if points.len() < 3 {
        result.push(PathFit {
            start: 0,
            end: points.len().saturating_sub(1),
            kind: PathFitKind::Linear,
        });
        return result;
    }
    let mut front = 0usize;
    let mut back = 0usize;
    let mut last_arc: Option<ArcSegment> = None;
    let mut current = Vec::new();
    for (i, &p) in points.iter().enumerate() {
        back = i;
        current.push(p);
        if back - front < 2 {
            continue;
        }
        let len_mm = polyline_length_mm(&current);
        if let Some(arc) = try_create_arc(&current, len_mm, tolerance_mm) {
            last_arc = Some(arc);
            if back == points.len() - 1 {
                result.push(arc_fit(front, back, arc));
                front = back;
            }
        } else if back - front > 2 {
            if let Some(arc) = last_arc {
                result.push(arc_fit(front, back - 1, arc));
            }
            front = back - 1;
            current.clear();
            current.push(points[front]);
            current.push(points[front + 1]);
        } else {
            if result
                .last()
                .is_none_or(|f| !matches!(f.kind, PathFitKind::Linear))
            {
                result.push(PathFit {
                    start: front,
                    end: front + 1,
                    kind: PathFitKind::Linear,
                });
            } else if let Some(last) = result.last_mut() {
                last.end = front + 1;
            }
            front = back - 1;
            current.clear();
            current.push(points[front]);
            current.push(points[front + 1]);
        }
    }
    if front != back {
        if result
            .last()
            .is_none_or(|f| !matches!(f.kind, PathFitKind::Linear))
        {
            result.push(PathFit {
                start: front,
                end: back,
                kind: PathFitKind::Linear,
            });
        } else if let Some(last) = result.last_mut() {
            last.end = back;
        }
    }
    result
}

fn arc_fit(start: usize, end: usize, arc: ArcSegment) -> PathFit {
    PathFit {
        start,
        end,
        kind: PathFitKind::Arc(arc),
    }
}

fn polyline_length_mm(pts: &[Point]) -> f64 {
    pts.windows(2).map(|w| w[0].distance_mm(w[1])).sum()
}

fn try_create_circle_3(p1: Point, p2: Point, p3: Point, max_radius_mm: f64) -> Option<Circle> {
    let (x1, y1) = p1.to_mm();
    let (x2, y2) = p2.to_mm();
    let (x3, y3) = p3.to_mm();
    if ((y1 - y2) * (x1 - x3) - (y1 - y3) * (x1 - x2)).abs() <= PARALLEL_AREA_MM2 {
        return None;
    }
    let a = x1 * (y2 - y3) - y1 * (x2 - x3) + x2 * y3 - x3 * y2;
    if a.abs() < CIRCLE_DET_EPS_MM2 {
        return None;
    }
    let b = (x1 * x1 + y1 * y1) * (y3 - y2)
        + (x2 * x2 + y2 * y2) * (y1 - y3)
        + (x3 * x3 + y3 * y3) * (y2 - y1);
    let c = (x1 * x1 + y1 * y1) * (x2 - x3)
        + (x2 * x2 + y2 * y2) * (x3 - x1)
        + (x3 * x3 + y3 * y3) * (x1 - x2);
    let center_x = -b / (2.0 * a);
    let center_y = -c / (2.0 * a);
    let radius = (center_x - x1).hypot(center_y - y1);
    if radius > max_radius_mm {
        return None;
    }
    Some(Circle {
        center_mm: (center_x, center_y),
        radius_mm: radius,
    })
}

fn try_create_circle(points: &[Point], max_radius_mm: f64, tolerance_mm: f64) -> Option<Circle> {
    let count = points.len();
    let middle = count / 2;
    if count == 3 {
        let c = try_create_circle_3(points[0], points[middle], points[count - 1], max_radius_mm)?;
        if !is_over_deviation(&c, points, tolerance_mm) {
            return Some(c);
        }
        return None;
    }
    let mid_pt = if count.is_multiple_of(2) {
        mid_point(points[middle], points[middle - 1])
    } else {
        mid_point(points[middle - 1], points[middle + 1])
    };
    if let Some(c) = try_create_circle_3(points[0], mid_pt, points[count - 1], max_radius_mm) {
        if !is_over_deviation(&c, points, tolerance_mm) {
            return Some(c);
        }
    }
    let mut best: Option<(Circle, f64)> = None;
    for index in 1..count - 1 {
        if index == middle {
            continue;
        }
        let Some(c) =
            try_create_circle_3(points[0], points[index], points[count - 1], max_radius_mm)
        else {
            continue;
        };
        let Some(dev) = deviation_sum_squared(&c, points, tolerance_mm) else {
            continue;
        };
        if best.as_ref().is_none_or(|(_, d)| dev < *d) {
            best = Some((c, dev));
        }
    }
    best.map(|(c, _)| c)
}

fn mid_point(a: Point, b: Point) -> Point {
    Point::new((a.x + b.x) / 2, (a.y + b.y) / 2)
}

fn is_over_deviation(c: &Circle, points: &[Point], tolerance_mm: f64) -> bool {
    for index in 0..points.len().saturating_sub(1) {
        if index != 0 {
            let (x, y) = points[index].to_mm();
            if ((x - c.center_mm.0).hypot(y - c.center_mm.1) - c.radius_mm).abs() > tolerance_mm {
                return true;
            }
        }
        if let Some(closest) = closest_perpendicular(points[index], points[index + 1], c.center_mm)
        {
            if ((closest.0 - c.center_mm.0).hypot(closest.1 - c.center_mm.1) - c.radius_mm).abs()
                > tolerance_mm
            {
                return true;
            }
        }
    }
    false
}

fn deviation_sum_squared(c: &Circle, points: &[Point], tolerance_mm: f64) -> Option<f64> {
    let mut total = 0.0;
    for p in points.iter().take(points.len().saturating_sub(1)).skip(1) {
        let (x, y) = p.to_mm();
        let deviation = ((x - c.center_mm.0).hypot(y - c.center_mm.1) - c.radius_mm).abs();
        total += deviation * deviation;
        if deviation > tolerance_mm {
            return None;
        }
    }
    for index in 0..points.len().saturating_sub(1) {
        if let Some(closest) = closest_perpendicular(points[index], points[index + 1], c.center_mm)
        {
            let deviation =
                ((closest.0 - c.center_mm.0).hypot(closest.1 - c.center_mm.1) - c.radius_mm).abs();
            total += deviation * deviation;
            if deviation > tolerance_mm {
                return None;
            }
        }
    }
    Some(total)
}

fn closest_perpendicular(p1: Point, p2: Point, c: (f64, f64)) -> Option<(f64, f64)> {
    let (x1, y1) = p1.to_mm();
    let (x2, y2) = p2.to_mm();
    let x_dif = x2 - x1;
    let y_dif = y2 - y1;
    let denom = x_dif * x_dif + y_dif * y_dif;
    if denom == 0.0 {
        return None;
    }
    let t = ((c.0 - x1) * x_dif + (c.1 - y1) * y_dif) / denom;
    if less_than_or_equal(t, 0.0) || greater_than_or_equal(t, 1.0) {
        return None;
    }
    Some((x1 + t * x_dif, y1 + t * y_dif))
}

fn less_than_or_equal(x: f64, y: f64) -> bool {
    x < y || (x - y).abs() < ZERO_TOLERANCE
}

fn greater_than_or_equal(x: f64, y: f64) -> bool {
    x > y || (x - y).abs() < ZERO_TOLERANCE
}

fn try_create_arc(
    points: &[Point],
    approximate_length_mm: f64,
    tolerance_mm: f64,
) -> Option<ArcSegment> {
    let c = try_create_circle(points, DEFAULT_MAX_RADIUS_MM, tolerance_mm)?;
    let mid = ((points.len() - 2) / 2) + 1;
    let arc = try_create_arc_from_circle(
        &c,
        points[0],
        points[mid],
        points[points.len() - 1],
        approximate_length_mm,
    )?;
    if are_points_within_slice(&arc, points) {
        Some(arc)
    } else {
        None
    }
}

fn try_create_arc_from_circle(
    c: &Circle,
    start: Point,
    mid: Point,
    end: Point,
    approximate_length_mm: f64,
) -> Option<ArcSegment> {
    let polar_start = c.polar(start);
    let polar_mid = c.polar(mid);
    let polar_end = c.polar(end);
    let mut angle = 0.0;
    let mut dir = None;
    if polar_end > polar_start {
        if polar_start < polar_mid && polar_mid < polar_end {
            dir = Some(ArcDir::Ccw);
            angle = polar_end - polar_start;
        } else if (0.0 <= polar_mid && polar_mid < polar_start)
            || (polar_end < polar_mid && polar_mid < std::f64::consts::TAU)
        {
            dir = Some(ArcDir::Cw);
            angle = polar_start + (std::f64::consts::TAU - polar_end);
        }
    } else if polar_start > polar_end {
        if (polar_start < polar_mid && polar_mid < std::f64::consts::TAU)
            || (0.0 < polar_mid && polar_mid < polar_end)
        {
            dir = Some(ArcDir::Ccw);
            angle = polar_end + (std::f64::consts::TAU - polar_start);
        } else if polar_end < polar_mid && polar_mid < polar_start {
            dir = Some(ArcDir::Cw);
            angle = polar_start - polar_end;
        }
    }
    let mut direction = dir?;
    if angle.abs() < EPSILON_MM {
        return None;
    }
    let mut arc_length = c.radius_mm * angle;
    let mut difference = (arc_length - approximate_length_mm) / approximate_length_mm;
    if difference.abs() >= PATH_TOLERANCE_PERCENT {
        let test_radians = (angle - std::f64::consts::TAU).abs();
        let test_arc_length = c.radius_mm * test_radians;
        difference = (test_arc_length - approximate_length_mm) / approximate_length_mm;
        if difference.abs() >= PATH_TOLERANCE_PERCENT {
            return None;
        }
        arc_length = test_arc_length;
        direction = match direction {
            ArcDir::Ccw => ArcDir::Cw,
            ArcDir::Cw => ArcDir::Ccw,
        };
    }
    Some(ArcSegment {
        center_mm: c.center_mm,
        radius_mm: c.radius_mm,
        start,
        end,
        length_mm: arc_length,
        dir: direction,
    })
}

fn are_points_within_slice(arc: &ArcSegment, points: &[Point]) -> bool {
    let point_count = points.len();
    if point_count < 2 {
        return false;
    }
    let c = Circle {
        center_mm: arc.center_mm,
        radius_mm: arc.radius_mm,
    };
    let polar_start = c.polar(arc.start);
    let polar_end = c.polar(arc.end);
    let start_mm = arc.start.to_mm();
    let end_mm = arc.end.to_mm();
    let start_norm = (
        (start_mm.0 - arc.center_mm.0) / arc.radius_mm,
        (start_mm.1 - arc.center_mm.1) / arc.radius_mm,
    );
    let end_norm = (
        (end_mm.0 - arc.center_mm.0) / arc.radius_mm,
        (end_mm.1 - arc.center_mm.1) / arc.radius_mm,
    );
    let will_cross_zero = match arc.dir {
        ArcDir::Ccw => polar_start > polar_end,
        ArcDir::Cw => polar_start < polar_end,
    };
    let mut previous_polar = polar_start;
    let mut crossed_zero = false;
    for index in (point_count - 2)..point_count {
        let polar_test = if index < point_count - 1 {
            c.polar(points[index])
        } else {
            polar_end
        };
        match arc.dir {
            ArcDir::Ccw => {
                if index < point_count - 1 {
                    let inside = if will_cross_zero {
                        polar_test > polar_start || polar_test < polar_end
                    } else {
                        polar_start < polar_test && polar_test < polar_end
                    };
                    if !inside {
                        return false;
                    }
                }
                if previous_polar > polar_test {
                    if !will_cross_zero || crossed_zero {
                        return false;
                    }
                    crossed_zero = true;
                }
            }
            ArcDir::Cw => {
                if index < point_count - 1 {
                    let inside = if will_cross_zero {
                        polar_test < polar_start || polar_test > polar_end
                    } else {
                        polar_start > polar_test && polar_test > polar_end
                    };
                    if !inside {
                        return false;
                    }
                }
                if previous_polar < polar_test {
                    if !will_cross_zero || crossed_zero {
                        return false;
                    }
                    crossed_zero = true;
                }
            }
        }
        let a = points[index - 1];
        let b = points[index];
        if (index != 1 && ray_intersects_segment(arc.center_mm, start_norm, a, b))
            || (index != point_count - 1 && ray_intersects_segment(arc.center_mm, end_norm, a, b))
        {
            return false;
        }
        previous_polar = polar_test;
    }
    will_cross_zero == crossed_zero
}

fn ray_intersects_segment(origin: (f64, f64), dir: (f64, f64), a: Point, b: Point) -> bool {
    let a_mm = a.to_mm();
    let b_mm = b.to_mm();
    let v1 = (origin.0 - a_mm.0, origin.1 - a_mm.1);
    let v2 = (b_mm.0 - a_mm.0, b_mm.1 - a_mm.1);
    let v3 = (-dir.1, dir.0);
    let dot = v2.0 * v3.0 + v2.1 * v3.1;
    if dot.abs() < EPSILON_MM {
        return false;
    }
    let t1 = (v2.0 * v1.1 - v2.1 * v1.0) / dot;
    let t2 = (v1.0 * v3.0 + v1.1 * v3.1) / dot;
    t1 >= 0.0 && (0.0..=1.0).contains(&t2)
}

/// C++ `Line::distance_to_squared` (scaled integer coords).
fn distance_to_squared(point: Point, a: Point, b: Point) -> f64 {
    let vx = (b.x - a.x) as f64;
    let vy = (b.y - a.y) as f64;
    let vax = (point.x - a.x) as f64;
    let vay = (point.y - a.y) as f64;
    let l2 = vx * vx + vy * vy;
    if l2 == 0.0 {
        return vax * vax + vay * vay;
    }
    let t = (vax * vx + vay * vy) / l2;
    if t <= 0.0 {
        vax * vax + vay * vay
    } else if t >= 1.0 {
        let dx = (point.x - b.x) as f64;
        let dy = (point.y - b.y) as f64;
        dx * dx + dy * dy
    } else {
        let dx = t * vx - vax;
        let dy = t * vy - vay;
        dx * dx + dy * dy
    }
}

/// C++ `ArcSegment::calc_arc_length` in the XY plane.
pub fn calc_arc_length_mm(
    start: (f64, f64),
    end: (f64, f64),
    center: (f64, f64),
    is_ccw: bool,
) -> f64 {
    let radius = (center.0 - start.0).hypot(center.1 - start.1);
    radius * calc_arc_radian(start, end, center, is_ccw)
}

fn calc_arc_radian(start: (f64, f64), end: (f64, f64), center: (f64, f64), is_ccw: bool) -> f64 {
    let d1 = (center.0 - start.0, center.1 - start.1);
    let d2 = (center.0 - end.0, center.1 - end.1);
    if (d1.0 - d2.0).hypot(d1.1 - d2.1) < 1e-6 {
        return std::f64::consts::TAU;
    }
    let dot = d1.0 * d2.0 + d1.1 * d2.1;
    let cross = d1.0 * d2.1 - d1.1 * d2.0;
    let mut radian = cross.atan2(dot);
    if is_ccw {
        if radian < 0.0 {
            radian += std::f64::consts::TAU;
        }
    } else if radian < 0.0 {
        radian = radian.abs();
    } else {
        radian = std::f64::consts::TAU - radian;
    }
    radian
}

#[cfg(test)]
mod tests {
    use super::*;

    fn semicircle(n: usize) -> Vec<Point> {
        (0..=n)
            .map(|i| {
                let t = i as f64 / n as f64 * std::f64::consts::PI;
                Point::from_mm(10.0 + 10.0 * t.cos(), 10.0 + 10.0 * t.sin())
            })
            .collect()
    }

    #[test]
    fn semicircle_fits_one_ccw_arc() {
        let pts = semicircle(48);
        let fits = do_arc_fitting(&pts, 0.012);
        let arcs: Vec<_> = fits
            .iter()
            .filter(|f| matches!(f.kind, PathFitKind::Arc(_)))
            .collect();
        assert!(
            !arcs.is_empty(),
            "expected an arc on a 48-point semicircle, got {fits:?}"
        );
        assert!(fits.iter().any(|f| matches!(
            f.kind,
            PathFitKind::Arc(ArcSegment {
                dir: ArcDir::Ccw,
                ..
            })
        )));
    }

    #[test]
    fn square_stays_linear() {
        let pts = vec![
            Point::from_mm(0.0, 0.0),
            Point::from_mm(10.0, 0.0),
            Point::from_mm(10.0, 10.0),
            Point::from_mm(0.0, 10.0),
            Point::from_mm(0.0, 0.0),
        ];
        let fits = do_arc_fitting(&pts, 0.012);
        assert!(
            fits.iter().all(|f| matches!(f.kind, PathFitKind::Linear)),
            "{fits:?}"
        );
    }

    #[test]
    fn douglas_peucker_drops_collinear() {
        let pts = vec![
            Point::from_mm(0.0, 0.0),
            Point::from_mm(5.0, 0.0),
            Point::from_mm(10.0, 0.0),
        ];
        let out = douglas_peucker(&pts, 0.012);
        assert_eq!(out.len(), 2);
        assert_eq!(out[0], pts[0]);
        assert_eq!(out[1], pts[2]);
    }
}
