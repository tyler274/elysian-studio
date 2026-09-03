//! Lightning infill (CuraEngine / Bambu `FillLightning`).
//!
//! Trees grow from the top of the sparse volume downward. Unsupported overhang
//! (sparse here, inset by a 45° wall radius, minus sparse above) becomes leaves
//! that attach to the nearest outline or existing tree. Branches copy to the
//! layer below so the same XY tree supports every slab under the skins.

use bambu_config::SliceSettings;
use bambu_geom::{difference_polygons, offset_polygons, scale, Point, Polygon, Polyline};

use super::{bbox, clip_to_region};
use crate::clip::point_in_polygons;

const CELL_PER_RADIUS: i64 = 6;
const MIN_CELL_MM: f64 = 0.05;

pub fn generate_layers(sparse: &[Vec<Polygon>], settings: &SliceSettings) -> Vec<Vec<Polyline>> {
    let n = sparse.len();
    let mut out = vec![Vec::new(); n];
    if n == 0 || settings.infill_density <= 0.0 {
        return out;
    }
    let params = Params::from_settings(settings);
    if params.supporting <= 0 || params.wall <= 0 {
        return out;
    }

    let overhangs = internal_overhangs(sparse, settings.layer_height_mm);
    let mut forests = vec![Forest::default(); n];

    for layer_id in (0..n).rev() {
        grow_trees(
            &mut forests[layer_id],
            &overhangs[layer_id],
            &sparse[layer_id],
            params,
        );
        if layer_id == 0 {
            break;
        }
        forests[layer_id - 1] = propagate(&forests[layer_id], &sparse[layer_id - 1]);
    }

    for (i, forest) in forests.iter().enumerate() {
        out[i] = clip_to_region(forest.to_polylines(), &sparse[i]);
    }
    out
}

#[derive(Clone, Copy)]
struct Params {
    supporting: i64,
    wall: i64,
    supporting2: i128,
}

impl Params {
    fn from_settings(settings: &SliceSettings) -> Self {
        let supporting = scale(settings.infill_spacing_mm().max(MIN_CELL_MM));
        let wall = scale(settings.layer_height_mm.max(MIN_CELL_MM));
        Self {
            supporting,
            wall,
            supporting2: sq(supporting),
        }
    }
}

#[derive(Clone, Default)]
struct Forest {
    nodes: Vec<Node>,
}

#[derive(Clone)]
struct Node {
    p: Point,
    parent: Option<usize>,
}

impl Forest {
    fn add(&mut self, p: Point, parent: Option<usize>) -> usize {
        let i = self.nodes.len();
        self.nodes.push(Node { p, parent });
        i
    }

    fn to_polylines(&self) -> Vec<Polyline> {
        self.nodes
            .iter()
            .filter_map(|node| {
                let parent = node.parent?;
                Some(vec![self.nodes[parent].p, node.p])
            })
            .collect()
    }
}

enum Ground {
    Boundary(Point),
    Node(usize),
}

impl Ground {
    fn p(&self, forest: &Forest) -> Point {
        match *self {
            Self::Boundary(p) => p,
            Self::Node(i) => forest.nodes[i].p,
        }
    }
}

fn internal_overhangs(sparse: &[Vec<Polygon>], layer_height_mm: f64) -> Vec<Vec<Polygon>> {
    let wall_mm = layer_height_mm.max(MIN_CELL_MM);
    let mut overhangs = vec![Vec::new(); sparse.len()];
    let mut above: Vec<Polygon> = Vec::new();
    for (i, here) in sparse.iter().enumerate().rev() {
        let supported = offset_polygons(here, -wall_mm);
        overhangs[i] = difference_polygons(&supported, &above);
        above = here.clone();
    }
    overhangs
}

fn grow_trees(forest: &mut Forest, overhang: &[Polygon], outlines: &[Polygon], params: Params) {
    if overhang.is_empty() || outlines.is_empty() {
        return;
    }
    let mut field = DistanceField::sample(overhang, outlines, params.supporting);
    let eps2 = sq(scale(MIN_CELL_MM));
    while let Some(idx) = field.next() {
        let leaf = field.cells[idx].loc;
        field.erased[idx] = true;
        let Some(ground) = best_ground(leaf, outlines, forest, params) else {
            continue;
        };
        let root = ground.p(forest);
        if dist_sq(root, leaf) < eps2 {
            field.erase_near_point(leaf, params.supporting2);
            continue;
        }
        match ground {
            Ground::Boundary(p) => {
                let parent = forest.add(p, None);
                forest.add(leaf, Some(parent));
            }
            Ground::Node(i) => {
                forest.add(leaf, Some(i));
            }
        }
        field.erase_near_segment(root, leaf, params.supporting2);
    }
}

fn best_ground(
    leaf: Point,
    outlines: &[Polygon],
    forest: &Forest,
    params: Params,
) -> Option<Ground> {
    let boundary = closest_on_polygons(leaf, outlines)?;
    let boundary2 = dist_sq(leaf, boundary);
    if boundary2 < sq(params.wall) || forest.nodes.is_empty() {
        return Some(Ground::Boundary(boundary));
    }

    let mut best = Ground::Boundary(boundary);
    let mut best2 = boundary2;
    for (i, node) in forest.nodes.iter().enumerate() {
        let d2 = dist_sq(leaf, node.p);
        if d2 >= best2 {
            continue;
        }
        if line_hits_outline(leaf, node.p, outlines) {
            continue;
        }
        best2 = d2;
        best = Ground::Node(i);
    }
    Some(best)
}

fn propagate(src: &Forest, below: &[Polygon]) -> Forest {
    let mut dst = Forest::default();
    if below.is_empty() || src.nodes.is_empty() {
        return dst;
    }

    let inside: Vec<bool> = src
        .nodes
        .iter()
        .map(|n| point_in_polygons(n.p, below))
        .collect();
    let mut keep = inside.clone();
    for (i, node) in src.nodes.iter().enumerate() {
        if !inside[i] {
            continue;
        }
        let mut parent = node.parent;
        while let Some(pi) = parent {
            if keep[pi] {
                break;
            }
            keep[pi] = true;
            parent = src.nodes[pi].parent;
        }
    }

    let mut map = vec![None; src.nodes.len()];
    for (i, node) in src.nodes.iter().enumerate() {
        if !keep[i] {
            continue;
        }
        let p = if inside[i] {
            node.p
        } else {
            closest_on_polygons(node.p, below).unwrap_or(node.p)
        };
        let parent = node.parent.and_then(|pi| map[pi]);
        map[i] = Some(dst.add(p, parent));
    }
    dst
}

struct Cell {
    loc: Point,
    dist_to_boundary2: i128,
}

struct DistanceField {
    cells: Vec<Cell>,
    erased: Vec<bool>,
    cursor: usize,
}

impl DistanceField {
    fn sample(overhang: &[Polygon], outlines: &[Polygon], supporting: i64) -> Self {
        let Some((min, max)) = bbox(overhang) else {
            return Self {
                cells: Vec::new(),
                erased: Vec::new(),
                cursor: 0,
            };
        };
        let cell = (supporting / CELL_PER_RADIUS).max(scale(MIN_CELL_MM));
        let mut cells = Vec::new();
        let mut y = min.y;
        while y <= max.y {
            let mut x = min.x;
            while x <= max.x {
                let loc = Point::new(x, y);
                if point_in_polygons(loc, overhang) {
                    let dist_to_boundary2 = closest_on_polygons(loc, outlines)
                        .map(|p| dist_sq(loc, p))
                        .unwrap_or(0);
                    cells.push(Cell {
                        loc,
                        dist_to_boundary2,
                    });
                }
                x = x.saturating_add(cell);
            }
            y = y.saturating_add(cell);
        }
        cells.sort_by_key(|c| c.dist_to_boundary2);
        let erased = vec![false; cells.len()];
        Self {
            cells,
            erased,
            cursor: 0,
        }
    }

    fn next(&mut self) -> Option<usize> {
        while self.cursor < self.erased.len() {
            let i = self.cursor;
            self.cursor += 1;
            if !self.erased[i] {
                return Some(i);
            }
        }
        None
    }

    fn erase_near_point(&mut self, p: Point, radius2: i128) {
        for (i, cell) in self.cells.iter().enumerate() {
            if !self.erased[i] && dist_sq(cell.loc, p) <= radius2 {
                self.erased[i] = true;
            }
        }
    }

    fn erase_near_segment(&mut self, a: Point, b: Point, radius2: i128) {
        for (i, cell) in self.cells.iter().enumerate() {
            if !self.erased[i] && dist_sq_to_segment(cell.loc, a, b) <= radius2 {
                self.erased[i] = true;
            }
        }
    }
}

fn closest_on_polygons(p: Point, polygons: &[Polygon]) -> Option<Point> {
    let mut best: Option<(i128, Point)> = None;
    for poly in polygons {
        let n = poly.len();
        if n < 2 {
            continue;
        }
        for i in 0..n {
            let a = poly[i];
            let b = poly[(i + 1) % n];
            let q = closest_on_segment(p, a, b);
            let d2 = dist_sq(p, q);
            if best.is_none_or(|(bd, _)| d2 < bd) {
                best = Some((d2, q));
            }
        }
    }
    best.map(|(_, q)| q)
}

fn closest_on_segment(p: Point, a: Point, b: Point) -> Point {
    let abx = (b.x - a.x) as i128;
    let aby = (b.y - a.y) as i128;
    let ab2 = abx * abx + aby * aby;
    if ab2 == 0 {
        return a;
    }
    let t = ((p.x - a.x) as i128 * abx + (p.y - a.y) as i128 * aby).clamp(0, ab2);
    Point::new(a.x + (abx * t / ab2) as i64, a.y + (aby * t / ab2) as i64)
}

fn dist_sq_to_segment(p: Point, a: Point, b: Point) -> i128 {
    dist_sq(p, closest_on_segment(p, a, b))
}

fn dist_sq(a: Point, b: Point) -> i128 {
    let dx = (a.x - b.x) as i128;
    let dy = (a.y - b.y) as i128;
    dx * dx + dy * dy
}

fn sq(v: i64) -> i128 {
    let v = v as i128;
    v * v
}

fn line_hits_outline(a: Point, b: Point, outlines: &[Polygon]) -> bool {
    outlines.iter().any(|poly| {
        let n = poly.len();
        n >= 2
            && (0..n).any(|i| {
                let c = poly[i];
                let d = poly[(i + 1) % n];
                segments_intersect(a, b, c, d)
            })
    })
}

fn cross(a: Point, b: Point, c: Point) -> i128 {
    (b.x - a.x) as i128 * (c.y - a.y) as i128 - (b.y - a.y) as i128 * (c.x - a.x) as i128
}

fn segments_intersect(p1: Point, p2: Point, q1: Point, q2: Point) -> bool {
    let d1 = cross(p1, p2, q1);
    let d2 = cross(p1, p2, q2);
    let d3 = cross(q1, q2, p1);
    let d4 = cross(q1, q2, p2);
    ((d1 > 0 && d2 < 0) || (d1 < 0 && d2 > 0)) && ((d3 > 0 && d4 < 0) || (d3 < 0 && d4 > 0))
}
