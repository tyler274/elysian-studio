//! Fuzzy skin (`FuzzySkin.cpp`): jitter wall polylines along their normals.
//!
//! Classic displacement only. C++ uses a thread-local RNG; we seed from the
//! layer index, slice Z, and the first point so the same mesh slices the same
//! way twice. Perlin / extrusion-width modes are not implemented yet.

use bambu_config::{FuzzySkinType, SliceSettings};
use bambu_geom::{Point, Polyline};

use crate::clip;

struct Rng(u64);

impl Rng {
    fn new(layer_idx: usize, z_mm: f64, poly: &[Point]) -> Self {
        let mut h =
            0x9E37_79B9_7F4A_7C15u64 ^ (layer_idx as u64).wrapping_mul(0xBF58_476D_1CE4_E5B9);
        h ^= z_mm.to_bits();
        if let Some(p) = poly.first() {
            h ^= p.x as u64;
            h = h.rotate_left(17) ^ (p.y as u64);
        }
        Self(h | 1)
    }

    fn unit(&mut self) -> f64 {
        self.0 = self.0.wrapping_mul(0x9E37_79B9_7F4A_7C15).wrapping_add(1);
        (self.0 >> 11) as f64 / ((1u64 << 53) as f64)
    }
}

/// C++ `fuzzy_polyline` for a closed ring (last→first is an edge).
fn fuzzy_closed(
    poly: &[Point],
    thickness_mm: f64,
    point_distance_mm: f64,
    rng: &mut Rng,
) -> Vec<Point> {
    if poly.len() < 3 || thickness_mm < 1e-9 || point_distance_mm < 1e-9 {
        return poly.to_vec();
    }
    let min_dist = point_distance_mm * 0.75;
    let range = point_distance_mm * 0.5;
    let mut dist_left = rng.unit() * (min_dist * 0.5);
    let mut out = Vec::new();
    let mut prev = poly[poly.len() - 1].to_mm();
    for p1 in poly {
        let cur = p1.to_mm();
        let dx = cur.0 - prev.0;
        let dy = cur.1 - prev.1;
        let size = (dx * dx + dy * dy).sqrt();
        if size < 1e-12 {
            prev = cur;
            continue;
        }
        let nx = -dy / size;
        let ny = dx / size;
        let mut d = dist_left;
        while d < size {
            let t = d / size;
            let px = prev.0 + dx * t;
            let py = prev.1 + dy * t;
            let r = (rng.unit() * 2.0 - 1.0) * thickness_mm;
            out.push(Point::from_mm(px + nx * r, py + ny * r));
            d += min_dist + rng.unit() * range;
        }
        dist_left = d - size;
        prev = cur;
    }
    if out.len() < 3 {
        poly.to_vec()
    } else {
        out
    }
}

fn ring_is_hole(idx: usize, rings: &[Polyline]) -> bool {
    let Some(ring) = rings.get(idx) else {
        return false;
    };
    if ring.len() < 3 {
        return false;
    }
    let n = ring.len() as i64;
    let c = Point::new(
        ring.iter().map(|p| p.x).sum::<i64>() / n,
        ring.iter().map(|p| p.y).sum::<i64>() / n,
    );
    clip::point_in_polygons_skip(c, rings, idx)
}

fn should_fuzzify(
    kind: FuzzySkinType,
    first_layer: bool,
    layer_idx: usize,
    is_inner: bool,
    is_hole: bool,
) -> bool {
    if !kind.is_enabled() {
        return false;
    }
    if !first_layer && layer_idx == 0 {
        return false;
    }
    match kind {
        FuzzySkinType::None => false,
        FuzzySkinType::External => !is_inner && !is_hole,
        FuzzySkinType::All => !is_inner,
        FuzzySkinType::AllWalls => true,
    }
}

pub fn apply_walls(
    outer: &mut [Polyline],
    inner: &mut [Polyline],
    settings: &SliceSettings,
    layer_idx: usize,
    z_mm: f64,
) {
    if !settings.fuzzy_skin.is_enabled() {
        return;
    }
    let thickness = settings.fuzzy_skin_thickness_mm;
    let spacing = settings.fuzzy_skin_point_distance_mm;
    let holes: Vec<bool> = (0..outer.len()).map(|i| ring_is_hole(i, outer)).collect();
    for (i, ring) in outer.iter_mut().enumerate() {
        if !should_fuzzify(
            settings.fuzzy_skin,
            settings.fuzzy_skin_first_layer,
            layer_idx,
            false,
            holes[i],
        ) {
            continue;
        }
        let mut rng = Rng::new(layer_idx, z_mm, ring);
        *ring = fuzzy_closed(ring, thickness, spacing, &mut rng);
    }
    if settings.fuzzy_skin != FuzzySkinType::AllWalls {
        return;
    }
    if !should_fuzzify(
        settings.fuzzy_skin,
        settings.fuzzy_skin_first_layer,
        layer_idx,
        true,
        false,
    ) {
        return;
    }
    for ring in inner.iter_mut() {
        let mut rng = Rng::new(layer_idx.wrapping_add(1_000_003), z_mm, ring);
        *ring = fuzzy_closed(ring, thickness, spacing, &mut rng);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use bambu_geom::Point;

    #[test]
    fn square_gains_vertices() {
        let square = vec![
            Point::from_mm(0.0, 0.0),
            Point::from_mm(20.0, 0.0),
            Point::from_mm(20.0, 20.0),
            Point::from_mm(0.0, 20.0),
        ];
        let mut rng = Rng::new(3, 1.0, &square);
        let out = fuzzy_closed(&square, 0.3, 0.8, &mut rng);
        assert!(out.len() > square.len() * 10, "got {}", out.len());
    }

    #[test]
    fn same_seed_is_stable() {
        let square = vec![
            Point::from_mm(0.0, 0.0),
            Point::from_mm(20.0, 0.0),
            Point::from_mm(20.0, 20.0),
            Point::from_mm(0.0, 20.0),
        ];
        let a = fuzzy_closed(&square, 0.3, 0.8, &mut Rng::new(3, 1.0, &square));
        let b = fuzzy_closed(&square, 0.3, 0.8, &mut Rng::new(3, 1.0, &square));
        assert_eq!(a, b);
    }
}
