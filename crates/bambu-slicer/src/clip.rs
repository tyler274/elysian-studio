//! Point-in-polygon and open-polyline clipping.

use bambu_geom::{Point, Polygon, Polyline};

pub fn point_in_polygon(poly: &[Point], p: Point) -> bool {
    let n = poly.len();
    if n < 3 {
        return false;
    }
    let mut inside = false;
    let mut j = n - 1;
    for i in 0..n {
        let a = poly[i];
        let b = poly[j];
        if (a.y > p.y) != (b.y > p.y) {
            let dy = b.y - a.y;
            if dy != 0 {
                let xints = a.x as i128 + (b.x - a.x) as i128 * (p.y - a.y) as i128 / dy as i128;
                if (p.x as i128) < xints {
                    inside = !inside;
                }
            }
        }
        j = i;
    }
    inside
}

pub fn point_in_polygons(p: Point, polygons: &[Polygon]) -> bool {
    let mut inside = false;
    for poly in polygons {
        if point_in_polygon(poly, p) {
            inside = !inside;
        }
    }
    inside
}

pub fn point_in_polygons_skip(p: Point, polygons: &[Polygon], skip: usize) -> bool {
    let mut inside = false;
    for (i, poly) in polygons.iter().enumerate() {
        if i == skip {
            continue;
        }
        if point_in_polygon(poly, p) {
            inside = !inside;
        }
    }
    inside
}

fn lerp(a: Point, b: Point, t: f64) -> Point {
    Point::new(
        a.x + ((b.x - a.x) as f64 * t).round() as i64,
        a.y + ((b.y - a.y) as f64 * t).round() as i64,
    )
}

/// Keep the portions of an open polyline that lie inside `polygons`.
pub fn clip_polyline(path: &[Point], polygons: &[Polygon]) -> Vec<Polyline> {
    if path.len() < 2 || polygons.is_empty() {
        return Vec::new();
    }

    let mut densified = Vec::new();
    for w in path.windows(2) {
        densified.push(w[0]);
        let dist2 = (w[1].x - w[0].x).abs().max((w[1].y - w[0].y).abs());
        let steps = (dist2 / 200_000).clamp(1, 32); // ~0.2mm
        for s in 1..steps {
            densified.push(lerp(w[0], w[1], s as f64 / steps as f64));
        }
    }
    densified.push(*path.last().unwrap());

    let flags: Vec<bool> = densified
        .iter()
        .map(|p| point_in_polygons(*p, polygons))
        .collect();

    let mut out = Vec::new();
    let mut current = Vec::new();
    for (p, inside) in densified.into_iter().zip(flags) {
        if inside {
            current.push(p);
        } else if current.len() >= 2 {
            out.push(std::mem::take(&mut current));
        } else {
            current.clear();
        }
    }
    if current.len() >= 2 {
        out.push(current);
    }
    out
}

/// Open or closed path split into overhang-degree runs (0 = supported, 5 = 100%).
#[derive(Debug, Clone)]
pub struct ClassifiedPath {
    pub path: Polyline,
    pub degree: u8,
}

impl ClassifiedPath {
    /// True when the run is not a 100% overhang (`erOverhangPerimeter`).
    pub fn inside(&self) -> bool {
        self.degree < 5
    }
}

/// Classify each edge by whether its midpoint lies in `polygons`.
///
/// Fully supported or fully overhanging loops keep the original vertices so
/// G-code point density does not change on vertical walls.
pub fn classify_polyline(
    path: &[Point],
    polygons: &[Polygon],
    closed: bool,
) -> Vec<ClassifiedPath> {
    classify_by(path, closed, |mid| {
        if polygons.is_empty() {
            5
        } else if point_in_polygons(mid, polygons) {
            0
        } else {
            5
        }
    })
}

/// C++ `detect_floating_line`: densify long edges so a run can change at the
/// sparse-area boundary instead of classifying a whole 20 mm segment as one.
pub fn classify_floating(
    path: &[Point],
    floating_areas: &[Polygon],
    closed: bool,
) -> Vec<ClassifiedPath> {
    classify_polyline(&densify_polyline(path, closed), floating_areas, closed)
}

fn densify_polyline(path: &[Point], closed: bool) -> Polyline {
    if path.len() < 2 {
        return path.to_vec();
    }
    let n = path.len();
    let edges = if closed { n } else { n - 1 };
    let mut out = Vec::new();
    for i in 0..edges {
        let a = path[i];
        let b = path[(i + 1) % n];
        if out.last() != Some(&a) {
            out.push(a);
        }
        let dist = (b.x - a.x).abs().max((b.y - a.y).abs());
        let steps = (dist / 200_000).clamp(1, 64);
        for s in 1..steps {
            out.push(lerp(a, b, s as f64 / steps as f64));
        }
    }
    if !closed {
        if let Some(&last) = path.last() {
            if out.last() != Some(&last) {
                out.push(last);
            }
        }
    }
    out
}

/// Classify edges against inset→grown lower-layer rings (C++ overhang degrees 0–5).
///
/// `rings[0]` is the most inset (degree 0 if inside); `rings.last()` is grown by
/// half the nozzle (outside that is degree 5).
pub fn classify_overhang(
    path: &[Point],
    rings: &[Vec<Polygon>],
    closed: bool,
) -> Vec<ClassifiedPath> {
    if rings.is_empty() {
        return classify_by(path, closed, |_| 0);
    }
    classify_by(path, closed, |mid| edge_degree(mid, rings))
}

fn edge_degree(mid: Point, rings: &[Vec<Polygon>]) -> u8 {
    for (i, ring) in rings.iter().enumerate() {
        if point_in_polygons(mid, ring) {
            return u8::try_from(i).unwrap_or(4);
        }
    }
    5
}

fn classify_by(path: &[Point], closed: bool, class: impl Fn(Point) -> u8) -> Vec<ClassifiedPath> {
    if path.len() < 2 {
        return Vec::new();
    }
    let n = path.len();
    let edge_count = if closed { n } else { n - 1 };
    let mut flags = Vec::with_capacity(edge_count);
    for i in 0..edge_count {
        let a = path[i];
        let b = path[(i + 1) % n];
        let mid = Point::new(a.x.midpoint(b.x), a.y.midpoint(b.y));
        flags.push(class(mid));
    }
    if flags.iter().all(|&f| f == flags[0]) {
        return vec![ClassifiedPath {
            path: path.to_vec(),
            degree: flags[0],
        }];
    }
    let mut runs: Vec<ClassifiedPath> = Vec::new();
    let mut start = 0usize;
    for i in 1..edge_count {
        if flags[i] != flags[start] {
            runs.push(edge_run(path, start, i, flags[start]));
            start = i;
        }
    }
    runs.push(edge_run(path, start, edge_count, flags[start]));
    if closed && runs.len() > 1 && runs.first().map(|r| r.degree) == runs.last().map(|r| r.degree) {
        let mut last = runs.pop().unwrap();
        let mut first = runs.remove(0);
        last.path.pop();
        last.path.append(&mut first.path);
        runs.insert(0, last);
    }
    runs
}

fn edge_run(path: &[Point], start: usize, end: usize, degree: u8) -> ClassifiedPath {
    let n = path.len();
    let mut pts = Vec::with_capacity(end - start + 1);
    for i in start..=end {
        pts.push(path[i % n]);
    }
    ClassifiedPath { path: pts, degree }
}

pub fn clip_polylines(paths: &[Polyline], polygons: &[Polygon]) -> Vec<Polyline> {
    paths
        .iter()
        .flat_map(|p| clip_polyline(p, polygons))
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    use bambu_geom::scale;

    fn square(size: f64) -> Polygon {
        let h = scale(size / 2.0);
        vec![
            Point::new(-h, -h),
            Point::new(h, -h),
            Point::new(h, h),
            Point::new(-h, h),
        ]
    }

    #[test]
    fn fully_inside_keeps_original_loop() {
        let support = square(20.0);
        let path = square(10.0);
        let runs = classify_polyline(&path, &[support], true);
        assert_eq!(runs.len(), 1);
        assert!(runs[0].inside());
        assert_eq!(runs[0].degree, 0);
        assert_eq!(runs[0].path.len(), 4);
    }

    #[test]
    fn fully_outside_is_one_overhang_run() {
        let support = square(4.0);
        let path = square(20.0);
        let runs = classify_polyline(&path, &[support], true);
        assert_eq!(runs.len(), 1);
        assert!(!runs[0].inside());
        assert_eq!(runs[0].degree, 5);
        assert_eq!(runs[0].path.len(), 4);
    }

    #[test]
    fn concentric_rings_pick_degree_2() {
        let rings = vec![
            vec![square(8.0)],
            vec![square(12.0)],
            vec![square(16.0)],
            vec![square(20.0)],
            vec![square(24.0)],
        ];
        let path = square(14.0);
        let runs = classify_overhang(&path, &rings, true);
        assert_eq!(runs.len(), 1);
        assert_eq!(runs[0].degree, 2);
        assert!(runs[0].inside());
    }

    #[test]
    fn crossing_line_splits_floating_and_supported() {
        let sparse = square(20.0);
        let path = vec![Point::new(scale(-20.0), 0), Point::new(scale(20.0), 0)];
        let runs = classify_floating(&path, &[sparse], false);
        assert!(
            runs.len() >= 2,
            "expected a split at the sparse boundary, got {runs:?}"
        );
        assert!(runs.iter().any(|r| r.degree == 0), "{runs:?}");
        assert!(runs.iter().any(|r| r.degree == 5), "{runs:?}");
    }
}
