#![forbid(unsafe_code)]

use serde::{Deserialize, Serialize};

/// Minimal FFF settings used by the vertical-slice pipeline.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct SliceSettings {
    pub layer_height_mm: f64,
    pub line_width_mm: f64,
    pub infill_density: f64,
    pub nozzle_diameter_mm: f64,
    pub filament_diameter_mm: f64,
    pub temperature_c: u16,
    pub bed_temperature_c: u16,
    pub print_speed_mm_s: f64,
    pub travel_speed_mm_s: f64,
}

impl Default for SliceSettings {
    fn default() -> Self {
        Self {
            layer_height_mm: 0.2,
            line_width_mm: 0.42,
            infill_density: 0.20,
            nozzle_diameter_mm: 0.4,
            filament_diameter_mm: 1.75,
            temperature_c: 220,
            bed_temperature_c: 60,
            print_speed_mm_s: 50.0,
            travel_speed_mm_s: 120.0,
        }
    }
}

impl SliceSettings {
    pub fn infill_spacing_mm(&self) -> f64 {
        if self.infill_density <= 0.0 {
            f64::INFINITY
        } else {
            (self.line_width_mm / self.infill_density).max(self.line_width_mm)
        }
    }
}
