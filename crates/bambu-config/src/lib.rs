#![forbid(unsafe_code)]

mod bbl;

use serde::{Deserialize, Serialize};

pub use bbl::{
    bbl_oracle_paths, bbl_resources_dir, flatten_bbl_profile, load_bbl_process,
    write_flattened_bbl_profile, BblOraclePaths, ConfigError,
};

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
pub enum InfillPattern {
    Rectilinear,
    Grid,
    Concentric,
    #[default]
    Gyroid,
    Honeycomb,
    /// Slic3r / Bambu `ip3DHoneycomb` (truncated octahedron slices).
    Honeycomb3D,
    /// CuraEngine / Bambu `ipLightning` (overhang trees, sparse below skins).
    Lightning,
    /// Bambu / PrusaSlicer `ipAdaptiveCubic` (octree densified at the mesh).
    AdaptiveCubic,
    /// Bambu / PrusaSlicer `ipSupportCubic` (octree densified under overhangs).
    SupportCubic,
}

impl InfillPattern {
    pub fn from_name(name: &str) -> Option<Self> {
        Some(match name.to_ascii_lowercase().as_str() {
            "rectilinear" | "line" => Self::Rectilinear,
            "grid" => Self::Grid,
            "concentric" => Self::Concentric,
            "gyroid" => Self::Gyroid,
            "honeycomb" | "hexagon" => Self::Honeycomb,
            "3dhoneycomb" | "3d honeycomb" | "3d_honeycomb" => Self::Honeycomb3D,
            "lightning" | "lightninginfill" | "lightning_infill" => Self::Lightning,
            "adaptivecubic" | "adaptive" | "adaptive_cubic" | "adaptive cubic" => {
                Self::AdaptiveCubic
            }
            "supportcubic" | "support_cubic" | "support cubic" => Self::SupportCubic,
            "zigzag" | "zig-zag" => Self::Rectilinear,
            _ => return None,
        })
    }

    pub fn as_str(self) -> &'static str {
        match self {
            Self::Rectilinear => "rectilinear",
            Self::Grid => "grid",
            Self::Concentric => "concentric",
            Self::Gyroid => "gyroid",
            Self::Honeycomb => "honeycomb",
            Self::Honeycomb3D => "3dhoneycomb",
            Self::Lightning => "lightning",
            Self::AdaptiveCubic => "adaptivecubic",
            Self::SupportCubic => "supportcubic",
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
pub enum SeamPosition {
    #[default]
    Aligned,
    Rear,
    Nearest,
    Random,
}

impl SeamPosition {
    pub fn from_name(name: &str) -> Option<Self> {
        Some(match name.to_ascii_lowercase().as_str() {
            "aligned" => Self::Aligned,
            "rear" | "back" => Self::Rear,
            "nearest" => Self::Nearest,
            "random" => Self::Random,
            _ => return None,
        })
    }
}

/// Classic offset walls. Arachne is not implemented yet.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
pub enum WallGenerator {
    #[default]
    Classic,
}

/// C++ `IroningType` (`ironing_type`).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
pub enum IroningType {
    #[default]
    NoIroning,
    TopSurfaces,
    TopmostOnly,
    AllSolid,
}

impl IroningType {
    pub fn from_name(name: &str) -> Option<Self> {
        Some(match name.to_ascii_lowercase().as_str() {
            "no ironing" | "none" | "off" | "no" => Self::NoIroning,
            "top" | "top surfaces" | "top_surfaces" => Self::TopSurfaces,
            "topmost" | "topmost surface" | "topmost_only" => Self::TopmostOnly,
            "solid" | "all solid" | "all_solid" | "all solid layer" => Self::AllSolid,
            _ => return None,
        })
    }
}

/// C++ ironing fill (`ironing_pattern`: concentric / zig-zag).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
pub enum IroningPattern {
    #[default]
    Rectilinear,
    Concentric,
}

impl IroningPattern {
    pub fn from_name(name: &str) -> Option<Self> {
        Some(match name.to_ascii_lowercase().as_str() {
            "concentric" => Self::Concentric,
            "zig-zag" | "zigzag" | "rectilinear" | "line" => Self::Rectilinear,
            _ => return None,
        })
    }
}

/// C++ `top_surface_pattern` / `bottom_surface_pattern`.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
pub enum SurfacePattern {
    #[default]
    Rectilinear,
    /// Same-direction scanlines (`ipMonotonic`).
    Monotonic,
    /// Monotonic without perimeter anchors (`ipMonotonicLine`, BBL default top).
    MonotonicLine,
    Concentric,
}

impl SurfacePattern {
    pub fn from_name(name: &str) -> Option<Self> {
        Some(match name.to_ascii_lowercase().as_str() {
            "rectilinear" | "line" | "zigzag" | "zig-zag" => Self::Rectilinear,
            "monotonic" => Self::Monotonic,
            "monotonicline" | "monotonic_line" | "monotonic line" => Self::MonotonicLine,
            "concentric" => Self::Concentric,
            _ => return None,
        })
    }

    pub fn as_str(self) -> &'static str {
        match self {
            Self::Rectilinear => "rectilinear",
            Self::Monotonic => "monotonic",
            Self::MonotonicLine => "monotonicline",
            Self::Concentric => "concentric",
        }
    }

    pub fn is_monotonic(self) -> bool {
        matches!(self, Self::Monotonic | Self::MonotonicLine)
    }
}

/// FFF settings used by the slice pipeline.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct SliceSettings {
    pub layer_height_mm: f64,
    pub first_layer_height_mm: f64,
    /// C++ `min_layer_height` (default 0.07). Used to probe the next slab and
    /// to clamp [`Self::precise_z_height`] adjustments.
    pub min_layer_height_mm: f64,
    /// C++ `max_layer_height` (default 3/4 nozzle).
    pub max_layer_height_mm: f64,
    /// Bambu `precise_z_height`: retune the last five slabs to the object top.
    pub precise_z_height: bool,
    /// First-layer inward offset (`elefant_foot_compensation`). 0 disables.
    pub elephant_foot_mm: f64,
    /// Grow/shrink outer contours (`xy_contour_compensation`). 0 disables.
    pub xy_contour_compensation_mm: f64,
    /// Enlarge/shrink holes (`xy_hole_compensation`). Positive makes holes bigger.
    pub xy_hole_compensation_mm: f64,
    pub line_width_mm: f64,
    pub wall_loops: u32,
    pub infill_density: f64,
    pub infill_pattern: InfillPattern,
    pub seam: SeamPosition,
    pub wall_generator: WallGenerator,
    pub nozzle_diameter_mm: f64,
    pub filament_diameter_mm: f64,
    pub flow_ratio: f64,
    pub temperature_c: u16,
    pub bed_temperature_c: u16,
    pub print_speed_mm_s: f64,
    pub infill_speed_mm_s: f64,
    pub travel_speed_mm_s: f64,
    pub support_speed_mm_s: f64,
    /// Skirt loops around layer 0 (0 disables).
    pub skirt_loops: u32,
    /// Gap between the outermost brim (or the object) and the innermost skirt loop.
    pub skirt_distance_mm: f64,
    /// Outer brim width on layer 0 (0 disables). Ignored when [`Self::raft_layers`] > 0.
    pub brim_width_mm: f64,
    /// Support-style layers under the object (`raft_layers`). 0 disables.
    pub raft_layers: u32,
    /// Air gap between raft contact and the first object layer (`raft_contact_distance`).
    pub raft_contact_distance_mm: f64,
    /// XY expansion of raft layers above the first (`raft_expansion`).
    pub raft_expansion_mm: f64,
    /// First raft layer XY expansion (`raft_first_layer_expansion`). Negative means auto (2 mm).
    pub raft_first_layer_expansion_mm: f64,
    /// First raft layer fill fraction (`raft_first_layer_density`, C++ percent).
    pub raft_first_layer_density: f64,
    pub enable_support: bool,
    /// Maximum overhang angle from vertical that does not need support (degrees).
    pub support_threshold_angle_deg: f64,
    pub support_density: f64,
    pub support_xy_distance_mm: f64,
    pub support_top_z_distance_mm: f64,
    pub support_interface_layers: u32,
    /// Solid layers at the bottom of the part (0 disables).
    pub bottom_shell_layers: u32,
    /// Solid layers at the top of the part (0 disables).
    pub top_shell_layers: u32,
    /// C++ `top_surface_pattern` (BBL 0.20 is `monotonicline`).
    pub top_surface_pattern: SurfacePattern,
    /// C++ `bottom_surface_pattern` (BBL common is `monotonic`).
    pub bottom_surface_pattern: SurfacePattern,
    pub solid_infill_speed_mm_s: f64,
    pub ironing_type: IroningType,
    pub ironing_pattern: IroningPattern,
    /// Fraction of normal layer height (C++ `ironing_flow` percent).
    pub ironing_flow: f64,
    pub ironing_spacing_mm: f64,
    /// Inset from the ironed contour. `0` means half the nozzle diameter.
    pub ironing_inset_mm: f64,
    pub ironing_speed_mm_s: f64,
}

impl Default for SliceSettings {
    fn default() -> Self {
        Self {
            layer_height_mm: 0.2,
            first_layer_height_mm: 0.2,
            min_layer_height_mm: 0.07,
            max_layer_height_mm: 0.3,
            precise_z_height: false,
            elephant_foot_mm: 0.0,
            xy_contour_compensation_mm: 0.0,
            xy_hole_compensation_mm: 0.0,
            line_width_mm: 0.42,
            wall_loops: 2,
            infill_density: 0.20,
            infill_pattern: InfillPattern::Gyroid,
            seam: SeamPosition::Aligned,
            wall_generator: WallGenerator::Classic,
            nozzle_diameter_mm: 0.4,
            filament_diameter_mm: 1.75,
            flow_ratio: 1.0,
            temperature_c: 220,
            bed_temperature_c: 60,
            print_speed_mm_s: 50.0,
            infill_speed_mm_s: 80.0,
            travel_speed_mm_s: 120.0,
            support_speed_mm_s: 80.0,
            skirt_loops: 2,
            skirt_distance_mm: 2.0,
            brim_width_mm: 0.0,
            raft_layers: 0,
            raft_contact_distance_mm: 0.1,
            raft_expansion_mm: 1.5,
            raft_first_layer_expansion_mm: -1.0,
            raft_first_layer_density: 0.90,
            enable_support: false,
            support_threshold_angle_deg: 30.0,
            support_density: 0.15,
            support_xy_distance_mm: 0.35,
            support_top_z_distance_mm: 0.2,
            support_interface_layers: 2,
            bottom_shell_layers: 3,
            top_shell_layers: 3,
            top_surface_pattern: SurfacePattern::Rectilinear,
            bottom_surface_pattern: SurfacePattern::Rectilinear,
            solid_infill_speed_mm_s: 80.0,
            ironing_type: IroningType::NoIroning,
            ironing_pattern: IroningPattern::Rectilinear,
            ironing_flow: 0.10,
            ironing_spacing_mm: 0.15,
            ironing_inset_mm: 0.21,
            ironing_speed_mm_s: 30.0,
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

    pub fn layer_height_at(&self, index: usize) -> f64 {
        if index == 0 {
            self.first_layer_height_mm
        } else {
            self.layer_height_mm
        }
    }

    /// First object slab height. With a raft, C++ consumes `initial_layer_print_height`
    /// on the raft flange and prints the object with `layer_height`.
    pub fn first_object_layer_height_mm(&self) -> f64 {
        if self.raft_layers > 0 {
            self.layer_height_mm.max(1e-6)
        } else {
            self.first_layer_height_mm.max(1e-6)
        }
    }

    pub fn support_spacing_mm(&self) -> f64 {
        if self.support_density <= 0.0 {
            f64::INFINITY
        } else {
            (self.line_width_mm / self.support_density).max(self.line_width_mm)
        }
    }

    /// Bambu `fdm_process_single_0.20` over `fdm_process_common`.
    pub fn bbl_0_20() -> Self {
        Self {
            infill_density: 0.15,
            infill_pattern: InfillPattern::Grid,
            skirt_loops: 0,
            brim_width_mm: 5.0,
            top_shell_layers: 5,
            top_surface_pattern: SurfacePattern::MonotonicLine,
            bottom_surface_pattern: SurfacePattern::Monotonic,
            elephant_foot_mm: 0.15,
            travel_speed_mm_s: 400.0,
            print_speed_mm_s: 200.0,
            infill_speed_mm_s: 270.0,
            solid_infill_speed_mm_s: 250.0,
            support_speed_mm_s: 80.0,
            ..Self::default()
        }
    }
}

/// Extrusion volume helpers (Slic3r `Flow`).
#[derive(Debug, Clone, Copy)]
pub struct Flow {
    pub width_mm: f64,
    pub height_mm: f64,
    pub filament_diameter_mm: f64,
    pub flow_ratio: f64,
}

impl Flow {
    pub fn from_settings(settings: &SliceSettings, height_mm: f64) -> Self {
        Self {
            width_mm: settings.line_width_mm,
            height_mm,
            filament_diameter_mm: settings.filament_diameter_mm,
            flow_ratio: settings.flow_ratio,
        }
    }

    pub fn mm3_per_mm(self) -> f64 {
        self.width_mm * self.height_mm * self.flow_ratio
    }

    pub fn e_per_mm(self) -> f64 {
        let filament_area = std::f64::consts::PI * (self.filament_diameter_mm * 0.5).powi(2);
        if filament_area <= 0.0 {
            0.0
        } else {
            self.mm3_per_mm() / filament_area
        }
    }
}
