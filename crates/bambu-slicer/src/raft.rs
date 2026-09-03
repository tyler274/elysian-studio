//! Raft under the object (`PrintObject::has_raft`, C++ `SlicingParameters`).
//!
//! Mesh slice planes stay object-relative. G-code / preview `print_z` is raised
//! by `object_print_z_min` (raft stack + `raft_contact_distance`). Brim is
//! skipped; the first raft layer is the bed flange (C++ `Brim.cpp` skips rafted
//! objects). Classic support columns are not merged into the raft yet.

use bambu_config::SliceSettings;
use bambu_geom::{offset_polygons, union_polygons, Polygon, Polyline};

use crate::infill;
use crate::Layer;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum RaftKind {
    Base,
    Interface,
    Contact,
}

#[derive(Debug, Clone, Copy)]
struct RaftSlab {
    height_mm: f64,
    print_z_mm: f64,
    kind: RaftKind,
}

/// C++ `SlicingParameters::object_print_z_min` (bottom of the first object slab).
pub(crate) fn object_print_z_min(settings: &SliceSettings) -> f64 {
    raft_slabs(settings)
        .last()
        .map(|s| s.print_z_mm + settings.raft_contact_distance_mm.max(0.0))
        .unwrap_or(0.0)
}

fn raft_slabs(settings: &SliceSettings) -> Vec<RaftSlab> {
    let n = settings.raft_layers as usize;
    if n == 0 {
        return Vec::new();
    }
    let h0 = settings.first_layer_height_mm.max(1e-6);
    if n == 1 {
        return vec![RaftSlab {
            height_mm: h0,
            print_z_mm: h0,
            kind: RaftKind::Contact,
        }];
    }

    // C++: `interface_raft_layers = (raft_layers + 1) / 2`, then
    // `base_raft_layers -= interface`. Contact is the last interface slab.
    let interface_n = n.div_ceil(2);
    let base_n = n - interface_n;
    let thick = settings
        .layer_height_mm
        .max(0.75 * settings.nozzle_diameter_mm);

    let mut slabs = Vec::with_capacity(n);
    let mut z = h0;
    slabs.push(RaftSlab {
        height_mm: h0,
        print_z_mm: z,
        kind: RaftKind::Base,
    });
    for _ in 1..base_n {
        z += thick;
        slabs.push(RaftSlab {
            height_mm: thick,
            print_z_mm: z,
            kind: RaftKind::Base,
        });
    }
    for _ in 1..interface_n {
        z += thick;
        slabs.push(RaftSlab {
            height_mm: thick,
            print_z_mm: z,
            kind: RaftKind::Interface,
        });
    }
    z += thick;
    slabs.push(RaftSlab {
        height_mm: thick,
        print_z_mm: z,
        kind: RaftKind::Contact,
    });
    slabs
}

fn first_layer_expansion_mm(settings: &SliceSettings) -> f64 {
    let auto_or_set = if settings.raft_first_layer_expansion_mm < 0.0 {
        2.0
    } else {
        settings.raft_first_layer_expansion_mm
    };
    auto_or_set.max(settings.raft_expansion_mm).max(0.0)
}

fn expand(src: &[Polygon], mm: f64) -> Vec<Polygon> {
    if mm < 1e-9 {
        return src.to_vec();
    }
    let grown = offset_polygons(src, mm);
    if grown.is_empty() {
        src.to_vec()
    } else {
        union_polygons(&grown)
    }
}

fn fill(region: &[Polygon], spacing_mm: f64, layer_index: usize, inset_mm: f64) -> Vec<Polyline> {
    let inset = offset_polygons(region, -inset_mm);
    let polys = if inset.is_empty() { region } else { &inset };
    infill::rectilinear(polys, spacing_mm, layer_index)
}

fn raft_layer(
    index: usize,
    slab: RaftSlab,
    contours: Vec<Polygon>,
    support: Vec<Polyline>,
    support_interface: Vec<Polyline>,
) -> Layer {
    let support_region = contours.clone();
    Layer {
        z_mm: slab.print_z_mm - 0.5 * slab.height_mm,
        index,
        height_mm: slab.height_mm,
        print_z_mm: slab.print_z_mm,
        contours,
        outer_walls: Vec::new(),
        inner_walls: Vec::new(),
        infill_region: Vec::new(),
        infill: Vec::new(),
        solid_infill: Vec::new(),
        top_surface: Vec::new(),
        bottom_surface: Vec::new(),
        bridge: Vec::new(),
        support,
        support_interface,
        support_region,
        skirt: Vec::new(),
        brim: Vec::new(),
        ironing: Vec::new(),
        top_region: Vec::new(),
        support_enforcer: Vec::new(),
        support_blocker: Vec::new(),
    }
}

pub fn apply(layers: &mut Vec<Layer>, settings: &SliceSettings) {
    let slabs = raft_slabs(settings);
    if slabs.is_empty() || layers.is_empty() {
        return;
    }

    let z_off = object_print_z_min(settings);
    for layer in layers.iter_mut() {
        layer.print_z_mm += z_off;
    }

    let object_outline = layers[0].contours.clone();
    let body = expand(&object_outline, settings.raft_expansion_mm.max(0.0));
    let flange = expand(&object_outline, first_layer_expansion_mm(settings));
    let inset = settings.line_width_mm * 0.5;
    let first_spacing = {
        let density = settings.raft_first_layer_density.clamp(0.10, 1.0);
        (settings.line_width_mm / density).max(settings.line_width_mm)
    };
    let interface_spacing = settings.line_width_mm * 1.1;
    let base_spacing = settings.support_spacing_mm();

    let mut raft = Vec::with_capacity(slabs.len());
    for (i, slab) in slabs.into_iter().enumerate() {
        let contours = if i == 0 { flange.clone() } else { body.clone() };
        let (support, support_interface) = if i == 0 {
            (fill(&contours, first_spacing, i, inset), Vec::new())
        } else {
            match slab.kind {
                RaftKind::Base => (fill(&contours, base_spacing, i, inset), Vec::new()),
                RaftKind::Interface | RaftKind::Contact => {
                    (Vec::new(), fill(&contours, interface_spacing, i, inset))
                }
            }
        };
        raft.push(raft_layer(i, slab, contours, support, support_interface));
    }

    let n_raft = raft.len();
    raft.append(layers);
    *layers = raft;
    for (i, layer) in layers.iter_mut().enumerate().skip(n_raft) {
        layer.index = i;
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use bambu_config::SliceSettings;

    #[test]
    fn one_raft_layer_is_first_print_height() {
        let mut settings = SliceSettings::default();
        settings.raft_layers = 1;
        let slabs = raft_slabs(&settings);
        assert_eq!(slabs.len(), 1);
        assert!((slabs[0].print_z_mm - 0.2).abs() < 1e-9);
        assert!((object_print_z_min(&settings) - 0.3).abs() < 1e-9);
    }

    #[test]
    fn two_raft_layers_split_base_and_contact() {
        let mut settings = SliceSettings::default();
        settings.raft_layers = 2;
        let slabs = raft_slabs(&settings);
        assert_eq!(slabs.len(), 2);
        assert_eq!(slabs[0].kind, RaftKind::Base);
        assert_eq!(slabs[1].kind, RaftKind::Contact);
        assert!((slabs[0].print_z_mm - 0.2).abs() < 1e-9);
        // contact height = max(layer_height, 0.75 * nozzle) = 0.3
        assert!((slabs[1].print_z_mm - 0.5).abs() < 1e-9);
        assert!((object_print_z_min(&settings) - 0.6).abs() < 1e-9);
    }
}
