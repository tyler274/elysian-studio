//! Seam placement on closed loops (aligned / rear).

use bambu_config::SeamPosition;
use bambu_geom::{Point, Polyline};

pub fn apply_seam(loop_pts: &mut Polyline, seam: SeamPosition, hint: Option<Point>) {
    if loop_pts.len() < 3 {
        return;
    }
    let idx = match seam {
        SeamPosition::Rear => loop_pts
            .iter()
            .enumerate()
            .max_by_key(|(_, p)| (p.y, -p.x))
            .map(|(i, _)| i)
            .unwrap_or(0),
        SeamPosition::Nearest | SeamPosition::Aligned => {
            let target = hint.unwrap_or_else(|| {
                loop_pts
                    .iter()
                    .copied()
                    .max_by_key(|p| (p.y, -p.x))
                    .unwrap_or(Point::new(0, 0))
            });
            nearest_index(loop_pts, target)
        }
        SeamPosition::Random => {
            let n = loop_pts.len();
            (loop_pts[0].x.unsigned_abs() as usize)
                .saturating_mul(1103515245)
                .wrapping_add(12345)
                % n
        }
    };
    rotate_to(loop_pts, idx);
}

fn nearest_index(loop_pts: &[Point], target: Point) -> usize {
    loop_pts
        .iter()
        .enumerate()
        .min_by_key(|(_, p)| {
            let dx = p.x - target.x;
            let dy = p.y - target.y;
            dx.saturating_mul(dx).saturating_add(dy.saturating_mul(dy))
        })
        .map(|(i, _)| i)
        .unwrap_or(0)
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
        apply_seam(&mut square, SeamPosition::Rear, None);
        assert_eq!(square[0].to_mm().1, 10.0);
    }
}
