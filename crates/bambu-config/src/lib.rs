#![forbid(unsafe_code)]

mod bbl;

use serde::{Deserialize, Serialize};

pub use bbl::{
    apply_config_pairs, bbl_oracle_paths, bbl_resources_dir, flatten_bbl_profile, is_region_key,
    load_bbl_process, overlay_bbl_profile, project_settings_json, settings_from_json,
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

    pub fn as_str(self) -> &'static str {
        match self {
            Self::Aligned => "aligned",
            Self::Rear => "rear",
            Self::Nearest => "nearest",
            Self::Random => "random",
        }
    }
}

/// C++ `wall_generator` (`classic` offset onions vs `arachne`).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
pub enum WallGenerator {
    /// Constant-width offset loops (`PerimeterGenerator::process_classic`).
    #[default]
    Classic,
    /// Classic full-width loops plus a centerline in leftover thinner than one wall.
    Arachne,
}

impl WallGenerator {
    pub fn from_name(name: &str) -> Option<Self> {
        Some(match name.to_ascii_lowercase().as_str() {
            "classic" | "classicwall" | "classic wall" => Self::Classic,
            "arachne" | "arachne-lite" | "variable" => Self::Arachne,
            _ => return None,
        })
    }

    pub fn as_str(self) -> &'static str {
        match self {
            Self::Classic => "classic",
            Self::Arachne => "arachne",
        }
    }
}

/// C++ `support_type` (`normal(auto)` vs `tree(auto)`).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
pub enum SupportType {
    /// Downward-projected columns (`normal(auto)` / `normal(manual)`).
    #[default]
    Classic,
    /// Slim trees to the bed (`tree(auto)` / `tree(manual)`). Bambu default.
    Tree,
}

impl SupportType {
    pub fn from_name(name: &str) -> Option<Self> {
        let n = name.to_ascii_lowercase();
        Some(match n.as_str() {
            "normal" | "normal(auto)" | "normal(manual)" | "classic" | "grid" => Self::Classic,
            "tree" | "tree(auto)" | "tree(manual)" | "organic" | "slim" => Self::Tree,
            _ => return None,
        })
    }

    pub fn as_str(self) -> &'static str {
        match self {
            Self::Classic => "normal(auto)",
            Self::Tree => "tree(auto)",
        }
    }
}

/// C++ `TopOneWallType` (`top_one_wall_type`, legacy `only_one_wall_top`).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
pub enum TopOneWallType {
    #[default]
    None,
    /// One wall on every region not covered by the layer above (`all top`).
    AllTop,
    /// One wall only on the last layer (`topmost`).
    Topmost,
}

impl TopOneWallType {
    pub fn from_name(name: &str) -> Option<Self> {
        Some(match name.to_ascii_lowercase().as_str() {
            "not apply" | "none" | "off" | "0" | "false" => Self::None,
            "all top" | "alltop" | "all" | "top" | "1" | "true" => Self::AllTop,
            "topmost" | "topmost surface" | "topmost_only" => Self::Topmost,
            _ => return None,
        })
    }

    pub fn as_str(self) -> &'static str {
        match self {
            Self::None => "not apply",
            Self::AllTop => "all top",
            Self::Topmost => "topmost",
        }
    }
}

/// C++ `FuzzySkinType` (`fuzzy_skin`).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
pub enum FuzzySkinType {
    #[default]
    None,
    /// Outer contours only (`external`).
    External,
    /// Outer contours and holes (`all`).
    All,
    /// Every wall loop (`allwalls`).
    AllWalls,
}

impl FuzzySkinType {
    pub fn from_name(name: &str) -> Option<Self> {
        Some(match name.to_ascii_lowercase().as_str() {
            "none" | "off" | "0" | "false" | "disabled" | "disabled_fuzzy" => Self::None,
            "external" | "contour" | "outer" => Self::External,
            "all" | "contour and hole" | "holes" => Self::All,
            "allwalls" | "all walls" | "all_walls" => Self::AllWalls,
            _ => return None,
        })
    }

    pub fn as_str(self) -> &'static str {
        match self {
            Self::None => "none",
            Self::External => "external",
            Self::All => "all",
            Self::AllWalls => "allwalls",
        }
    }

    pub fn is_enabled(self) -> bool {
        !matches!(self, Self::None)
    }
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

    pub fn as_str(self) -> &'static str {
        match self {
            Self::NoIroning => "no ironing",
            Self::TopSurfaces => "top",
            Self::TopmostOnly => "topmost",
            Self::AllSolid => "solid",
        }
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

    pub fn as_str(self) -> &'static str {
        match self {
            Self::Rectilinear => "zig-zag",
            Self::Concentric => "concentric",
        }
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
    /// C++ `top_one_wall_type` (BBL default is `all top`).
    pub top_one_wall: TopOneWallType,
    pub infill_density: f64,
    pub infill_pattern: InfillPattern,
    pub seam: SeamPosition,
    pub wall_generator: WallGenerator,
    /// C++ `min_feature_size` as a fraction of nozzle diameter (default 25%).
    pub min_feature_size: f64,
    /// C++ `min_bead_width` as a fraction of nozzle diameter (default 85%).
    pub min_bead_width: f64,
    /// C++ `fuzzy_skin` (BBL default is `none`).
    pub fuzzy_skin: FuzzySkinType,
    pub fuzzy_skin_thickness_mm: f64,
    pub fuzzy_skin_point_distance_mm: f64,
    /// Apply jitter on object layer 0 (`fuzzy_skin_first_layer`).
    pub fuzzy_skin_first_layer: bool,
    pub nozzle_diameter_mm: f64,
    pub filament_diameter_mm: f64,
    pub flow_ratio: f64,
    pub temperature_c: u16,
    pub bed_temperature_c: u16,
    pub print_speed_mm_s: f64,
    /// C++ `inner_wall_speed`. Defaults to the outer wall speed.
    pub inner_wall_speed_mm_s: f64,
    /// C++ `initial_layer_speed` (first printed layer walls and most features).
    pub first_layer_speed_mm_s: f64,
    /// C++ `initial_layer_infill_speed`.
    pub first_layer_infill_speed_mm_s: f64,
    pub infill_speed_mm_s: f64,
    pub travel_speed_mm_s: f64,
    pub support_speed_mm_s: f64,
    /// C++ `detect_overhang_wall`: clip walls against the layer below.
    pub detect_overhang_wall: bool,
    /// C++ `enable_overhang_speed`: slow unsupported wall segments.
    pub enable_overhang_speed: bool,
    /// C++ `overhang_1_4_speed`. 0 means keep the wall speed.
    pub overhang_1_4_speed_mm_s: f64,
    /// C++ `overhang_2_4_speed`.
    pub overhang_2_4_speed_mm_s: f64,
    /// C++ `overhang_3_4_speed`.
    pub overhang_3_4_speed_mm_s: f64,
    /// C++ `overhang_4_4_speed`.
    pub overhang_4_4_speed_mm_s: f64,
    /// C++ `overhang_totally_speed` (100% overhang / degree 5).
    pub overhang_speed_mm_s: f64,
    /// C++ `bridge_speed`.
    pub bridge_speed_mm_s: f64,
    /// C++ `top_surface_speed`.
    pub top_surface_speed_mm_s: f64,
    /// C++ `small_perimeter_speed` raw value (mm/s, or percent of outer wall).
    pub small_perimeter_speed: f64,
    /// When set, [`Self::small_perimeter_speed`] is a percent of outer wall speed.
    pub small_perimeter_speed_is_percent: bool,
    /// C++ `small_perimeter_threshold` (mm). Compared as a circle radius.
    pub small_perimeter_threshold_mm: f64,
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
    /// C++ `support_type`. Default is classic columns; BBL profiles use tree.
    pub support_type: SupportType,
    /// Maximum overhang angle from vertical that does not need support (degrees).
    pub support_threshold_angle_deg: f64,
    pub support_density: f64,
    pub support_xy_distance_mm: f64,
    pub support_top_z_distance_mm: f64,
    pub support_interface_layers: u32,
    /// Max XY lean per layer (`tree_support_branch_angle`).
    pub tree_branch_angle_deg: f64,
    /// Disk diameter at each tree node (`tree_support_branch_diameter`).
    pub tree_branch_diameter_mm: f64,
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
    /// C++ `default_acceleration` (mm/s²). Used by the G-code time estimator.
    pub default_acceleration_mm_s2: f64,
    /// C++ `travel_acceleration` (mm/s²).
    pub travel_acceleration_mm_s2: f64,
    /// C++ `filament_density` (g/cm³). Generic PLA is 1.24.
    pub filament_density_g_cm3: f64,
    /// C++ `fan_min_speed` (percent).
    pub fan_min_speed: u32,
    /// C++ `fan_max_speed` (percent).
    pub fan_max_speed: u32,
    /// C++ `close_fan_the_first_x_layers`.
    pub close_fan_the_first_x_layers: u32,
    /// C++ `first_x_layer_part_fan_speed` (percent). Default 0.
    pub first_x_layer_part_fan_speed: u32,
    /// C++ `full_fan_speed_layer`. 0 disables the ramp.
    pub full_fan_speed_layer: u32,
    /// C++ `fan_cooling_layer_time` (seconds).
    pub fan_cooling_layer_time_s: f64,
    /// C++ `slow_down_layer_time` (seconds). Also the full-fan time threshold.
    pub slow_down_layer_time_s: f64,
    /// C++ `reduce_fan_stop_start_freq` (keep fan at least at min speed).
    pub reduce_fan_stop_start_freq: bool,
    /// C++ `filament_max_volumetric_speed` (mm³/s). 0 disables the cap.
    pub filament_max_volumetric_speed_mm3_s: f64,
    /// C++ `machine_max_jerk_x` / `_y` (mm/s). X1 Carbon default is 9.
    pub xy_jerk_mm_s: f64,
    /// C++ `machine_max_jerk_z` (mm/s). X1 Carbon default is 3.
    pub z_jerk_mm_s: f64,
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
            top_one_wall: TopOneWallType::None,
            infill_density: 0.20,
            infill_pattern: InfillPattern::Gyroid,
            seam: SeamPosition::Aligned,
            wall_generator: WallGenerator::Classic,
            min_feature_size: 0.25,
            min_bead_width: 0.85,
            fuzzy_skin: FuzzySkinType::None,
            fuzzy_skin_thickness_mm: 0.3,
            fuzzy_skin_point_distance_mm: 0.8,
            fuzzy_skin_first_layer: false,
            nozzle_diameter_mm: 0.4,
            filament_diameter_mm: 1.75,
            flow_ratio: 1.0,
            temperature_c: 220,
            bed_temperature_c: 60,
            print_speed_mm_s: 50.0,
            inner_wall_speed_mm_s: 50.0,
            first_layer_speed_mm_s: 50.0,
            first_layer_infill_speed_mm_s: 80.0,
            infill_speed_mm_s: 80.0,
            travel_speed_mm_s: 120.0,
            support_speed_mm_s: 80.0,
            detect_overhang_wall: true,
            enable_overhang_speed: true,
            overhang_1_4_speed_mm_s: 0.0,
            overhang_2_4_speed_mm_s: 50.0,
            overhang_3_4_speed_mm_s: 30.0,
            overhang_4_4_speed_mm_s: 10.0,
            overhang_speed_mm_s: 10.0,
            bridge_speed_mm_s: 25.0,
            top_surface_speed_mm_s: 50.0,
            small_perimeter_speed: 50.0,
            small_perimeter_speed_is_percent: true,
            small_perimeter_threshold_mm: 0.0,
            skirt_loops: 2,
            skirt_distance_mm: 2.0,
            brim_width_mm: 0.0,
            raft_layers: 0,
            raft_contact_distance_mm: 0.1,
            raft_expansion_mm: 1.5,
            raft_first_layer_expansion_mm: -1.0,
            raft_first_layer_density: 0.90,
            enable_support: false,
            support_type: SupportType::Classic,
            support_threshold_angle_deg: 30.0,
            support_density: 0.15,
            support_xy_distance_mm: 0.35,
            support_top_z_distance_mm: 0.2,
            support_interface_layers: 2,
            tree_branch_angle_deg: 45.0,
            tree_branch_diameter_mm: 2.0,
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
            default_acceleration_mm_s2: 10000.0,
            travel_acceleration_mm_s2: 10000.0,
            filament_density_g_cm3: 1.24,
            fan_min_speed: 20,
            fan_max_speed: 100,
            close_fan_the_first_x_layers: 1,
            first_x_layer_part_fan_speed: 0,
            full_fan_speed_layer: 0,
            fan_cooling_layer_time_s: 60.0,
            slow_down_layer_time_s: 8.0,
            reduce_fan_stop_start_freq: false,
            filament_max_volumetric_speed_mm3_s: 0.0,
            xy_jerk_mm_s: 9.0,
            z_jerk_mm_s: 3.0,
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

    /// Features thinner than this are skipped (`min_feature_size` × nozzle).
    pub fn min_feature_size_mm(&self) -> f64 {
        let frac = if self.min_feature_size > 0.0 {
            self.min_feature_size
        } else {
            0.25
        };
        frac * self.nozzle_diameter_mm
    }

    /// C++ `small_perimeter_speed.get_abs_value(outer_wall_speed)`.
    pub fn small_perimeter_speed_mm_s(&self) -> f64 {
        if self.small_perimeter_speed_is_percent {
            self.small_perimeter_speed * self.print_speed_mm_s / 100.0
        } else {
            self.small_perimeter_speed
        }
    }

    /// Cap an extrusion feed (mm/min) with `filament_max_volumetric_speed`.
    pub fn cap_extrude_feed_mm_min(&self, print_f: f64, mm3_per_mm: f64) -> f64 {
        let max = self.filament_max_volumetric_speed_mm3_s;
        if max <= 0.0 || mm3_per_mm <= 1e-12 {
            print_f
        } else {
            print_f.min(max / mm3_per_mm * 60.0)
        }
    }

    /// Thin-feature extrusion floor (`min_bead_width` × nozzle).
    pub fn min_bead_width_mm(&self) -> f64 {
        let frac = if self.min_bead_width > 0.0 {
            self.min_bead_width
        } else {
            0.85
        };
        frac * self.nozzle_diameter_mm
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
            top_one_wall: TopOneWallType::AllTop,
            support_type: SupportType::Tree,
            travel_speed_mm_s: 400.0,
            print_speed_mm_s: 200.0,
            inner_wall_speed_mm_s: 300.0,
            first_layer_speed_mm_s: 50.0,
            first_layer_infill_speed_mm_s: 105.0,
            infill_speed_mm_s: 270.0,
            solid_infill_speed_mm_s: 250.0,
            support_speed_mm_s: 80.0,
            enable_overhang_speed: true,
            overhang_1_4_speed_mm_s: 0.0,
            overhang_2_4_speed_mm_s: 50.0,
            overhang_3_4_speed_mm_s: 30.0,
            overhang_4_4_speed_mm_s: 10.0,
            overhang_speed_mm_s: 10.0,
            bridge_speed_mm_s: 50.0,
            top_surface_speed_mm_s: 200.0,
            fan_min_speed: 100,
            fan_max_speed: 100,
            close_fan_the_first_x_layers: 1,
            fan_cooling_layer_time_s: 100.0,
            slow_down_layer_time_s: 8.0,
            reduce_fan_stop_start_freq: true,
            filament_max_volumetric_speed_mm3_s: 12.0,
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
