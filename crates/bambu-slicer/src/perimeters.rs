//! Classic offset perimeters (`PerimeterGenerator::process_classic`).
//!
//! `top_one_wall_type` / legacy `only_one_wall_top`: the topmost layer (and,
//! for `AllTop`, terraces not covered by the layer above) keep a single outer
//! wall so top infill can fill the rest. Extra inner walls continue only under
//! the layer above (C++ `generate_one_wall_by_top_most` / `Alltop`).

use bambu_config::{SliceSettings, TopOneWallType};
use bambu_geom::{
    difference_polygons, intersect_polygons, offset_polygons, union_polygons, Polygon, Polyline,
};

use crate::seams;

const COVER_MM: f64 = 0.15;

pub struct PerimeterResult {
    pub outer: Vec<Polyline>,
    pub inner: Vec<Polyline>,
    pub infill_region: Vec<Polygon>,
    pub seam_hint: Option<bambu_geom::Point>,
}

pub fn classic_perimeters(
    contours: &[Polygon],
    settings: &SliceSettings,
    seam_hint: Option<bambu_geom::Point>,
    upper: Option<&[Polygon]>,
) -> PerimeterResult {
    let w = settings.line_width_mm;
    let loops = settings.wall_loops.max(1);
    let upper = upper.filter(|u| !u.is_empty());
    let one_wall_layer =
        loops > 1 && settings.top_one_wall != TopOneWallType::None && upper.is_none();

    let mut hint = seam_hint;
    let (outer, mut inner) = if one_wall_layer {
        let (outer, hint_out) = onion_rings(contours, 1, w, settings, hint);
        hint = hint_out;
        (outer, Vec::new())
    } else {
        onion_split(contours, loops, w, settings, hint, &mut hint)
    };

    let wall_n = if one_wall_layer { 1 } else { loops };
    let mut infill_region = offset_polygons(contours, -w * (wall_n as f64 + 0.5));

    if !one_wall_layer && loops > 1 && settings.top_one_wall == TopOneWallType::AllTop {
        if let Some(upper) = upper {
            let cover = cover_upper(upper);
            let remaining = offset_polygons(contours, -w);
            let not_top = intersect_polygons(&remaining, &cover);
            let after_one = offset_polygons(contours, -w * 1.5);
            let top = difference_polygons(&after_one, &cover);
            if not_top.is_empty() {
                inner.clear();
                infill_region = after_one;
            } else {
                let extra = loops - 1;
                let (more, hint_out) = onion_rings(&not_top, extra, w, settings, hint);
                hint = hint_out;
                inner = more;
                infill_region = offset_polygons(&not_top, -w * (extra as f64 + 0.5));
                if !top.is_empty() {
                    infill_region.extend(top);
                    infill_region = union_polygons(&infill_region);
                }
            }
        }
    }

    PerimeterResult {
        outer,
        inner,
        infill_region,
        seam_hint: hint,
    }
}

fn cover_upper(upper: &[Polygon]) -> Vec<Polygon> {
    let grown = offset_polygons(upper, COVER_MM);
    if grown.is_empty() {
        union_polygons(upper)
    } else {
        union_polygons(&grown)
    }
}

fn onion_split(
    contours: &[Polygon],
    loops: u32,
    w: f64,
    settings: &SliceSettings,
    mut hint: Option<bambu_geom::Point>,
    hint_out: &mut Option<bambu_geom::Point>,
) -> (Vec<Polyline>, Vec<Polyline>) {
    let mut outer = Vec::new();
    let mut inner = Vec::new();
    for i in 0..loops {
        let rings = offset_loops(contours, w * (i as f64 + 0.5), settings, &mut hint);
        if i == 0 {
            outer.extend(rings);
        } else {
            inner.extend(rings);
        }
    }
    *hint_out = hint;
    (outer, inner)
}

fn onion_rings(
    contours: &[Polygon],
    loops: u32,
    w: f64,
    settings: &SliceSettings,
    mut hint: Option<bambu_geom::Point>,
) -> (Vec<Polyline>, Option<bambu_geom::Point>) {
    let mut out = Vec::new();
    for i in 0..loops {
        out.extend(offset_loops(
            contours,
            w * (i as f64 + 0.5),
            settings,
            &mut hint,
        ));
    }
    (out, hint)
}

fn offset_loops(
    contours: &[Polygon],
    inset_mm: f64,
    settings: &SliceSettings,
    hint: &mut Option<bambu_geom::Point>,
) -> Vec<Polyline> {
    let mut rings = offset_polygons(contours, -inset_mm);
    rings.retain(|r| r.len() >= 3);
    for ring in &mut rings {
        seams::apply_seam(ring, settings.seam, *hint);
        *hint = ring.first().copied();
    }
    rings
}
