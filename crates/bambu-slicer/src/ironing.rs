//! `PrintObjectStep::Ironing`: low-flow recross of top / solid shells.

use bambu_config::{IroningPattern, IroningType, SliceSettings};
use bambu_geom::offset_polygons;

use crate::infill;
use crate::Layer;

pub fn apply(layers: &mut [Layer], settings: &SliceSettings) {
    if layers.is_empty() || settings.ironing_type == IroningType::NoIroning {
        return;
    }

    let n = layers.len();
    let inset = if settings.ironing_inset_mm <= 0.0 {
        0.5 * settings.nozzle_diameter_mm
    } else {
        settings.ironing_inset_mm
    };
    let spacing = settings.ironing_spacing_mm.max(0.02);

    for (i, layer) in layers.iter_mut().enumerate() {
        let area = ironing_area(settings.ironing_type, i, n, layer);
        if area.is_empty() {
            continue;
        }
        let inset_area = offset_polygons(&area, -inset);
        if inset_area.is_empty() {
            continue;
        }
        layer.ironing = match settings.ironing_pattern {
            IroningPattern::Concentric => infill::concentric(&inset_area, spacing),
            IroningPattern::Rectilinear => infill::solid_monotonic(&inset_area, spacing, i),
        };
    }
}

fn ironing_area(kind: IroningType, i: usize, n: usize, layer: &Layer) -> Vec<bambu_geom::Polygon> {
    match kind {
        IroningType::NoIroning => Vec::new(),
        IroningType::TopSurfaces => layer.top_region.clone(),
        IroningType::TopmostOnly if i + 1 == n => {
            if layer.top_region.is_empty() {
                layer.infill_region.clone()
            } else {
                layer.top_region.clone()
            }
        }
        IroningType::TopmostOnly => Vec::new(),
        IroningType::AllSolid => {
            if layer.infill.is_empty() {
                layer.infill_region.clone()
            } else {
                layer.top_region.clone()
            }
        }
    }
}
