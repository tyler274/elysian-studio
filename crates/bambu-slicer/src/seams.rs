//! Seam placement on closed loops (aligned / rear).

use bambu_config::SeamPosition;
use bambu_geom::{Point, Polygon, Polyline};

use crate::clip::point_in_polygons;

pub fn apply_seam(
    loop_pts: &mut Polyline,
    seam: SeamPosition,
    hint: Option<Point>,
    supported: Option<&[Polygon]>,
) {
    if loop_pts.len() < 3 {
        return;
    }
    let idx = pick_seam_index(loop_pts, seam, hint, supported);
    rotate_to(loop_pts, idx);
}

/// C++ `seam_placement_away_from_overhangs`: re-pick aligned / rear / nearest
/// starts among vertices that sit over the layer below.
pub fn retime_away_from_overhangs(
    outer: &mut [Polyline],
    inner: &mut [Polyline],
    seam: SeamPosition,
    supported: &[Polygon],
) -> Option<Point> {
    if supported.is_empty() {
        return None;
    }
    let mut hint = None;
    for ring in outer.iter_mut().chain(inner.iter_mut()) {
        apply_seam(ring, seam, hint, Some(supported));
        hint = ring.first().copied();
    }
    hint
}

fn pick_seam_index(
    loop_pts: &Polyline,
    seam: SeamPosition,
    hint: Option<Point>,
    supported: Option<&[Polygon]>,
) -> usize {
    let n = loop_pts.len();
    let mut chosen = Vec::new();
    if let Some(polys) = supported {
        chosen.extend((0..n).filter(|&i| point_in_polygons(loop_pts[i], polys)));
    }
    if chosen.is_empty() {
        chosen.extend(0..n);
    }
    match seam {
        SeamPosition::Rear => chosen
            .into_iter()
            .max_by_key(|&i| (loop_pts[i].y, -loop_pts[i].x))
            .unwrap_or(0),
        SeamPosition::Nearest | SeamPosition::Aligned => {
            let target = hint.unwrap_or_else(|| {
                chosen
                    .iter()
                    .copied()
                    .map(|i| loop_pts[i])
                    .max_by_key(|p| (p.y, -p.x))
                    .unwrap_or(Point::new(0, 0))
            });
            chosen
                .into_iter()
                .min_by_key(|&i| {
                    let dx = loop_pts[i].x - target.x;
                    let dy = loop_pts[i].y - target.y;
                    dx.saturating_mul(dx).saturating_add(dy.saturating_mul(dy))
                })
                .unwrap_or(0)
        }
        SeamPosition::Random => {
            let seed = (loop_pts[0].x.unsigned_abs() as usize)
                .saturating_mul(1103515245)
                .wrapping_add(12345);
            chosen[seed % chosen.len()]
        }
    }
}

fn rotate_to(loop_pts: &mut Polyline, idx: usize) {
    if idx == 0 || idx >= loop_pts.len() {
        return;
    }
    loop_pts.rotate_left(idx);
}

#[cfg(test)]
mod tests {
    use super::*;
    use bambu_geom::Point;

    #[test]
    fn rear_seam_picks_highest_y() {
        let mut square = vec![
            Point::from_mm(0.0, 0.0),
            Point::from_mm(10.0, 0.0),
            Point::from_mm(10.0, 10.0),
            Point::from_mm(0.0, 10.0),
        ];
        apply_seam(&mut square, SeamPosition::Rear, None, None);
        assert_eq!(square[0].to_mm().1, 10.0);
    }

    #[test]
    fn rear_seam_skips_unsupported_overhang() {
        let mut loop_pts = vec![
            Point::from_mm(1.0, 1.0),
            Point::from_mm(9.0, 1.0),
            Point::from_mm(9.0, 9.0),
            Point::from_mm(5.0, 14.0),
            Point::from_mm(1.0, 9.0),
        ];
        let supported = [vec![
            Point::from_mm(0.0, 0.0),
            Point::from_mm(10.0, 0.0),
            Point::from_mm(10.0, 10.0),
            Point::from_mm(0.0, 10.0),
        ]];
        apply_seam(
            &mut loop_pts,
            SeamPosition::Rear,
            None,
            Some(supported.as_slice()),
        );
        let (x, y) = loop_pts[0].to_mm();
        assert!(
            y <= 9.0 + 1e-6,
            "overhang vertex y=14 should lose to the supported back, got ({x}, {y})"
        );
        assert!((y - 9.0).abs() < 1e-6);
        assert!(
            (x - 1.0).abs() < 1e-6,
            "rear among supported is min-x at y=9, got ({x}, {y})"
        );
    }
}
