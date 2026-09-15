//! Fuzzy skin (`FuzzySkin.cpp`): jitter wall polylines along their normals.
//!
//! C++ uses a thread-local RNG for classic noise and point spacing; we seed from
//! the layer index, slice Z, and the first point so the same mesh slices the same
//! way twice. Perlin / Billow / Ridged / Voronoi sample `(x, y, z)` instead.

use bambu_config::{FuzzySkinNoiseType, FuzzySkinType, SliceSettings};
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

fn fade(t: f64) -> f64 {
    t * t * t * (t * (t * 6.0 - 15.0) + 10.0)
}

fn lerp(a: f64, b: f64, t: f64) -> f64 {
    a + (b - a) * t
}

fn lattice_hash(ix: i32, iy: i32, iz: i32) -> f64 {
    let mut h = ix as i64;
    h = h.wrapping_mul(374761393).wrapping_add(iy as i64);
    h = h.wrapping_mul(668265263).wrapping_add(iz as i64);
    h ^= h >> 13;
    h = h.wrapping_mul(1274126177);
    let u = (h as u64) >> 11;
    (u as f64 / ((1u64 << 53) as f64)) * 2.0 - 1.0
}

fn value_noise(x: f64, y: f64, z: f64) -> f64 {
    let x0 = x.floor() as i32;
    let y0 = y.floor() as i32;
    let z0 = z.floor() as i32;
    let tx = fade(x - f64::from(x0));
    let ty = fade(y - f64::from(y0));
    let tz = fade(z - f64::from(z0));
    let n000 = lattice_hash(x0, y0, z0);
    let n100 = lattice_hash(x0 + 1, y0, z0);
    let n010 = lattice_hash(x0, y0 + 1, z0);
    let n110 = lattice_hash(x0 + 1, y0 + 1, z0);
    let n001 = lattice_hash(x0, y0, z0 + 1);
    let n101 = lattice_hash(x0 + 1, y0, z0 + 1);
    let n011 = lattice_hash(x0, y0 + 1, z0 + 1);
    let n111 = lattice_hash(x0 + 1, y0 + 1, z0 + 1);
    let x00 = lerp(n000, n100, tx);
    let x10 = lerp(n010, n110, tx);
    let x01 = lerp(n001, n101, tx);
    let x11 = lerp(n011, n111, tx);
    lerp(lerp(x00, x10, ty), lerp(x01, x11, ty), tz)
}

fn fbm(x: f64, y: f64, z: f64, octaves: u32, persistence: f64) -> f64 {
    let mut amp = 1.0;
    let mut freq = 1.0;
    let mut sum = 0.0;
    let mut norm = 0.0;
    for _ in 0..octaves.max(1) {
        sum += amp * value_noise(x * freq, y * freq, z * freq);
        norm += amp;
        amp *= persistence;
        freq *= 2.0;
    }
    if norm > 1e-12 {
        sum / norm
    } else {
        0.0
    }
}

fn voronoi(x: f64, y: f64, z: f64) -> f64 {
    let x0 = x.floor() as i32;
    let y0 = y.floor() as i32;
    let z0 = z.floor() as i32;
    let mut best = f64::MAX;
    for dz in -1..=1 {
        for dy in -1..=1 {
            for dx in -1..=1 {
                let cx = x0 + dx;
                let cy = y0 + dy;
                let cz = z0 + dz;
                let px = f64::from(cx) + 0.5 * (lattice_hash(cx, cy, cz) + 1.0);
                let py = f64::from(cy) + 0.5 * (lattice_hash(cx + 19, cy, cz) + 1.0);
                let pz = f64::from(cz) + 0.5 * (lattice_hash(cx, cy + 23, cz) + 1.0);
                let d = (px - x).hypot(py - y).hypot(pz - z);
                best = best.min(d);
            }
        }
    }
    (best * 2.0 - 1.0).clamp(-1.0, 1.0)
}

fn sample_noise(settings: &SliceSettings, x: f64, y: f64, z: f64, rng: &mut Rng) -> f64 {
    let scale = settings.fuzzy_skin_scale.max(0.01);
    let f = 1.0 / scale;
    let octaves = settings.fuzzy_skin_octaves;
    let persistence = settings.fuzzy_skin_persistence;
    match settings.fuzzy_skin_noise_type {
        FuzzySkinNoiseType::Classic => rng.unit() * 2.0 - 1.0,
        FuzzySkinNoiseType::Perlin => fbm(x * f, y * f, z * f, octaves, persistence),
        FuzzySkinNoiseType::Billow => {
            fbm(x * f, y * f, z * f, octaves, persistence).abs() * 2.0 - 1.0
        }
        FuzzySkinNoiseType::RidgedMulti => {
            1.0 - fbm(x * f, y * f, z * f, octaves, persistence).abs()
        }
        FuzzySkinNoiseType::Voronoi => voronoi(x * f, y * f, z * f),
    }
}

/// C++ `fuzzy_polyline` for a closed ring (last→first is an edge).
fn fuzzy_closed(poly: &[Point], settings: &SliceSettings, z_mm: f64, rng: &mut Rng) -> Vec<Point> {
    let thickness_mm = settings.fuzzy_skin_thickness_mm;
    let point_distance_mm = settings.fuzzy_skin_point_distance_mm;
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
            let r = sample_noise(settings, px, py, z_mm, rng) * thickness_mm;
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
        *ring = fuzzy_closed(ring, settings, z_mm, &mut rng);
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
        *ring = fuzzy_closed(ring, settings, z_mm, &mut rng);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use bambu_geom::Point;

    fn square() -> Vec<Point> {
        vec![
            Point::from_mm(0.0, 0.0),
            Point::from_mm(20.0, 0.0),
            Point::from_mm(20.0, 20.0),
            Point::from_mm(0.0, 20.0),
        ]
    }

    fn classic_settings() -> SliceSettings {
        let mut settings = SliceSettings::default();
        settings.fuzzy_skin_thickness_mm = 0.3;
        settings.fuzzy_skin_point_distance_mm = 0.8;
        settings
    }

    #[test]
    fn square_gains_vertices() {
        let poly = square();
        let settings = classic_settings();
        let mut rng = Rng::new(3, 1.0, &poly);
        let out = fuzzy_closed(&poly, &settings, 1.0, &mut rng);
        assert!(out.len() > poly.len() * 10, "got {}", out.len());
    }

    #[test]
    fn same_seed_is_stable() {
        let poly = square();
        let settings = classic_settings();
        let a = fuzzy_closed(&poly, &settings, 1.0, &mut Rng::new(3, 1.0, &poly));
        let b = fuzzy_closed(&poly, &settings, 1.0, &mut Rng::new(3, 1.0, &poly));
        assert_eq!(a, b);
    }

    #[test]
    fn perlin_differs_from_classic_and_is_stable() {
        let poly = square();
        let classic = classic_settings();
        let mut perlin = classic.clone();
        perlin.fuzzy_skin_noise_type = FuzzySkinNoiseType::Perlin;
        let a = fuzzy_closed(&poly, &classic, 1.0, &mut Rng::new(3, 1.0, &poly));
        let b = fuzzy_closed(&poly, &perlin, 1.0, &mut Rng::new(3, 1.0, &poly));
        let c = fuzzy_closed(&poly, &perlin, 1.0, &mut Rng::new(3, 1.0, &poly));
        assert_ne!(a, b, "Perlin should not match classic uniform noise");
        assert_eq!(b, c);
        assert!(b.len() > poly.len() * 10, "got {}", b.len());
    }
}
