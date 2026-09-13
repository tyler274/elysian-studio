#![forbid(unsafe_code)]

mod bbl;
mod placeholder;

use serde::{Deserialize, Serialize};

pub use bbl::{
    apply_config_pairs, bbl_oracle_paths, bbl_resources_dir, config_block_gcode,
    flatten_bbl_profile, is_region_key, list_bbl_profiles, load_bbl_process, overlay_bbl_profile,
    project_settings_json, settings_from_json, write_flattened_bbl_profile, BblOraclePaths,
    BblProfileEntry, BblProfileKind, ConfigError,
};
pub use placeholder::{expand_placeholders, PlaceholderContext};

/// C++ `SPARSE_INFILL_RESOLUTION` (mm). Sparse infill arc-fits coarser than walls.
pub const SPARSE_INFILL_RESOLUTION_MM: f64 = 0.04;
/// C++ `SUPPORT_RESOLUTION` (mm). Support arc-fits coarser than walls.
pub const SUPPORT_RESOLUTION_MM: f64 = 0.0375;
/// C++ `LOOP_CLIPPING_LENGTH_OVER_NOZZLE_DIAMETER` for concentric fill loops.
pub const LOOP_CLIPPING_OVER_NOZZLE: f64 = 0.15;
/// C++ `BRIDGE_EXTRA_SPACING` (mm). Thick-bridge line spacing is the circular
/// diameter plus this gap.
pub const BRIDGE_EXTRA_SPACING_MM: f64 = 0.05;

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
    /// Bambu `ip2DLattice`.
    Lattice2D,
    /// Bambu `ipLockedZag`.
    LockedZag,
    /// Bambu `ipCubic` (axis-aligned 3D cubic slices).
    Cubic,
    /// Bambu `ipTriangles`.
    Triangles,
    /// Bambu `ipStars`.
    Stars,
    /// Bambu `ipCrossHatch`.
    CrossHatch,
    /// Bambu `ipHilbertCurve`.
    Hilbert,
    /// Bambu `ipArchimedeanChords`.
    Archimedean,
    /// Bambu `ipOctagramSpiral`.
    Octagram,
    /// Bambu `ipCrossZag`.
    CrossZag,
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
            "2dlattice" | "2d lattice" | "lattice" => Self::Lattice2D,
            "lockedzag" | "locked zag" | "locked-zag" => Self::LockedZag,
            "cubic" => Self::Cubic,
            "triangles" => Self::Triangles,
            "stars" => Self::Stars,
            "crosshatch" | "cross-hatch" | "crosshatchinfill" => Self::CrossHatch,
            "hilbertcurve" | "hilbert" | "hilbertcurveinfill" => Self::Hilbert,
            "archimedeanchords" | "archimedean" | "archimedean_chords" => Self::Archimedean,
            "octagramspiral" | "octagram" | "octagram_spiral" => Self::Octagram,
            "crosszag" | "cross-zag" | "cross_zag" => Self::CrossZag,
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
            Self::Lattice2D => "2dlattice",
            Self::LockedZag => "lockedzag",
            Self::Cubic => "cubic",
            Self::Triangles => "triangles",
            Self::Stars => "stars",
            Self::CrossHatch => "crosshatch",
            Self::Hilbert => "hilbertcurve",
            Self::Archimedean => "archimedeanchords",
            Self::Octagram => "octagramspiral",
            Self::CrossZag => "crosszag",
        }
    }

    /// C++ `Fill.cpp` patterns that honor `fill_multiline` on sparse infill.
    pub fn supports_multiline(self) -> bool {
        !matches!(self, Self::Concentric)
    }
}

/// C++ `CoolingSlowdownLogicType`.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
pub enum CoolingSlowdownLogic {
    #[default]
    Uniform,
    ConsistentSurface,
}

impl CoolingSlowdownLogic {
    pub fn from_name(name: &str) -> Option<Self> {
        Some(match name.to_ascii_lowercase().as_str() {
            "uniform_cooling" | "uniform" => Self::Uniform,
            "consistent_surface" | "consistent" => Self::ConsistentSurface,
            _ => return None,
        })
    }

    pub fn as_str(self) -> &'static str {
        match self {
            Self::Uniform => "uniform_cooling",
            Self::ConsistentSurface => "consistent_surface",
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

/// C++ `SeamScarfType` (`seam_slope_type` / `filament_scarf_seam_type`).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
pub enum SeamScarfType {
    #[default]
    None,
    /// Outer contours (`external` / "Contour").
    External,
    /// Contours and holes (`all`).
    All,
}

impl SeamScarfType {
    pub fn from_name(name: &str) -> Option<Self> {
        Some(match name.to_ascii_lowercase().as_str() {
            "none" | "off" | "0" | "false" | "disabled" => Self::None,
            "external" | "contour" | "outer" => Self::External,
            "all" | "contour and hole" | "holes" => Self::All,
            _ => return None,
        })
    }

    pub fn as_str(self) -> &'static str {
        match self {
            Self::None => "none",
            Self::External => "external",
            Self::All => "all",
        }
    }

    pub fn is_enabled(self) -> bool {
        !matches!(self, Self::None)
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

/// C++ `WallSequence` (`wall_sequence`). Legacy `wall_infill_order` remaps here.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
pub enum WallSequence {
    /// Inner FEATURE bucket, then outer (`inner wall/outer wall`). C++ default.
    #[default]
    InnerOuter,
    /// Outer FEATURE bucket, then inner (`outer wall/inner wall`).
    OuterInner,
    /// Remaining inners, outer, then the first inner (`elrSecondPerimeter`).
    InnerOuterInner,
}

impl WallSequence {
    pub fn from_name(name: &str) -> Option<Self> {
        Some(match name.trim().to_ascii_lowercase().as_str() {
            "inner wall/outer wall" | "inner/outer" | "innerouter" => Self::InnerOuter,
            "outer wall/inner wall" | "outer/inner" | "outerinner" => Self::OuterInner,
            "inner-outer-inner wall" | "inner-outer-inner" | "inner wall/outer wall/inner wall" => {
                Self::InnerOuterInner
            }
            "inner wall/outer wall/infill" | "infill/inner wall/outer wall" => Self::InnerOuter,
            "outer wall/inner wall/infill" | "infill/outer wall/inner wall" => Self::OuterInner,
            "inner-outer-inner wall/infill" => Self::InnerOuterInner,
            _ => return None,
        })
    }

    pub fn as_str(self) -> &'static str {
        match self {
            Self::InnerOuter => "inner wall/outer wall",
            Self::OuterInner => "outer wall/inner wall",
            Self::InnerOuterInner => "inner-outer-inner wall",
        }
    }
}

/// C++ `DraftShield` (`draft_shield`).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
pub enum DraftShield {
    #[default]
    Disabled,
    /// Skirt height still follows [`SliceSettings::skirt_height`] (first layer if 0).
    Limited,
    /// Skirt is as tall as the object (`Print::has_infinite_skirt`).
    Enabled,
}

impl DraftShield {
    pub fn from_name(name: &str) -> Option<Self> {
        Some(match name.trim().to_ascii_lowercase().as_str() {
            "disabled" | "0" | "false" | "none" => Self::Disabled,
            "limited" => Self::Limited,
            "enabled" | "1" | "true" => Self::Enabled,
            _ => return None,
        })
    }

    pub fn as_str(self) -> &'static str {
        match self {
            Self::Disabled => "disabled",
            Self::Limited => "limited",
            Self::Enabled => "enabled",
        }
    }
}

/// C++ `BrimType` (`brim_type`). Default `auto_brim` uses [`SliceSettings::brim_width_mm`]
/// for outer loops (auto-width is not implemented). Public BBL hides inner brim.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
pub enum BrimType {
    #[default]
    AutoBrim,
    BrimEars,
    OuterOnly,
    InnerOnly,
    OuterAndInner,
    NoBrim,
}

impl BrimType {
    pub fn from_name(name: &str) -> Option<Self> {
        Some(match name.trim().to_ascii_lowercase().as_str() {
            "auto_brim" | "auto" => Self::AutoBrim,
            "brim_ears" | "painted" => Self::BrimEars,
            "outer_only" | "outer brim only" | "outer" => Self::OuterOnly,
            "inner_only" | "inner brim only" | "inner" => Self::InnerOnly,
            "outer_and_inner" | "outer and inner brim" => Self::OuterAndInner,
            "no_brim" | "no-brim" | "none" => Self::NoBrim,
            _ => return None,
        })
    }

    pub fn as_str(self) -> &'static str {
        match self {
            Self::AutoBrim => "auto_brim",
            Self::BrimEars => "brim_ears",
            Self::OuterOnly => "outer_only",
            Self::InnerOnly => "inner_only",
            Self::OuterAndInner => "outer_and_inner",
            Self::NoBrim => "no_brim",
        }
    }

    /// C++ outer brim: auto / ears / outer_only / outer_and_inner.
    pub fn has_outer(self) -> bool {
        !matches!(self, Self::NoBrim | Self::InnerOnly)
    }
}

/// C++ `OverhangFanThreshold` (`overhang_fan_threshold`).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
#[repr(u8)]
pub enum OverhangFanThreshold {
    /// `0%`: boost every outer wall.
    None = 0,
    /// `10%`.
    OneFour = 1,
    /// `25%`.
    TwoFour = 2,
    /// `50%`. Generic PLA.
    ThreeFour = 3,
    /// `75%`.
    FourFour = 4,
    /// `95%`: only 100% overhang walls and bridges. C++ default.
    #[default]
    Bridge = 5,
}

impl OverhangFanThreshold {
    pub fn from_name(name: &str) -> Option<Self> {
        Some(match name.trim() {
            "0%" | "0" | "none" => Self::None,
            "10%" | "10" | "1/4" => Self::OneFour,
            "25%" | "25" | "2/4" => Self::TwoFour,
            "50%" | "50" | "3/4" => Self::ThreeFour,
            "75%" | "75" | "4/4" => Self::FourFour,
            "95%" | "95" | "bridge" => Self::Bridge,
            _ => return None,
        })
    }

    pub fn as_str(self) -> &'static str {
        match self {
            Self::None => "0%",
            Self::OneFour => "10%",
            Self::TwoFour => "25%",
            Self::ThreeFour => "50%",
            Self::FourFour => "75%",
            Self::Bridge => "95%",
        }
    }

    fn compare_degree(self) -> u8 {
        match self {
            Self::None => 0,
            other => other as u8 - 1,
        }
    }
}

/// C++ `ZHopType`.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
pub enum ZHopType {
    Auto,
    Normal,
    Slope,
    #[default]
    Spiral,
}

impl ZHopType {
    pub fn from_name(name: &str) -> Option<Self> {
        Some(match name.trim().to_ascii_lowercase().as_str() {
            "auto lift" | "auto" => Self::Auto,
            "normal lift" | "normal" => Self::Normal,
            "slope lift" | "slope" => Self::Slope,
            "spiral lift" | "spiral" => Self::Spiral,
            _ => return None,
        })
    }

    pub fn as_str(self) -> &'static str {
        match self {
            Self::Auto => "Auto Lift",
            Self::Normal => "Normal Lift",
            Self::Slope => "Slope Lift",
            Self::Spiral => "Spiral Lift",
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

/// C++ `SupportMaterialPattern` (`support_base_pattern`).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
pub enum SupportBasePattern {
    /// Classic → rectilinear; tree → hollow (`smpDefault`).
    #[default]
    Default,
    Rectilinear,
    RectilinearGrid,
    Honeycomb,
    /// Classic remaps to rectilinear (`SupportParameters`).
    Lightning,
    /// No base fill; interface still prints (`smpNone` / `"hollow"`).
    None,
}

impl SupportBasePattern {
    pub fn from_name(name: &str) -> Option<Self> {
        Some(match name.to_ascii_lowercase().as_str() {
            "default" => Self::Default,
            "rectilinear" | "line" => Self::Rectilinear,
            "rectilinear-grid" | "rectilinear_grid" | "grid" => Self::RectilinearGrid,
            "honeycomb" | "hexagon" => Self::Honeycomb,
            "lightning" => Self::Lightning,
            "hollow" | "none" => Self::None,
            _ => return None,
        })
    }

    pub fn as_str(self) -> &'static str {
        match self {
            Self::Default => "default",
            Self::Rectilinear => "rectilinear",
            Self::RectilinearGrid => "rectilinear-grid",
            Self::Honeycomb => "honeycomb",
            Self::Lightning => "lightning",
            Self::None => "hollow",
        }
    }

    /// C++ classic `base_fill_pattern` after `smpDefault` / lightning remap.
    pub fn classic_fill(self) -> Self {
        match self {
            Self::Default | Self::Lightning => Self::Rectilinear,
            other => other,
        }
    }

    /// C++ tree remap: `smpDefault` / lightning → hollow (`smpNone`).
    pub fn tree_fill(self) -> Self {
        match self {
            Self::Default | Self::Lightning => Self::None,
            other => other,
        }
    }
}

/// C++ `SupportMaterialInterfacePattern` (`support_interface_pattern`).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
pub enum SupportInterfacePattern {
    /// BBL `"auto"`: rectilinear hatch (rewrite keeps 0° so defaults do not move).
    #[default]
    Auto,
    Rectilinear,
    Concentric,
    /// Alternate ±45° around C++ `interface_angle` (90° when `support_angle` is 0).
    RectilinearInterlaced,
    Grid,
}

impl SupportInterfacePattern {
    pub fn from_name(name: &str) -> Option<Self> {
        Some(match name.to_ascii_lowercase().as_str() {
            "auto" | "default" => Self::Auto,
            "rectilinear" | "line" => Self::Rectilinear,
            "concentric" => Self::Concentric,
            "rectilinear_interlaced" | "rectilinear-interlaced" | "interlaced" => {
                Self::RectilinearInterlaced
            }
            "grid" => Self::Grid,
            _ => return None,
        })
    }

    pub fn as_str(self) -> &'static str {
        match self {
            Self::Auto => "auto",
            Self::Rectilinear => "rectilinear",
            Self::Concentric => "concentric",
            Self::RectilinearInterlaced => "rectilinear_interlaced",
            Self::Grid => "grid",
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

/// C++ extrusion-role acceleration pick in `GCode::extrude`.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum PrintAccel {
    #[default]
    Default,
    OuterWall,
    InnerWall,
    TopSurface,
    SparseInfill,
}

/// C++ `FlowRole` used to pick extrusion width.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum FlowRole {
    ExternalPerimeter,
    Perimeter,
    SparseInfill,
    SolidInfill,
    TopSolidInfill,
    SupportMaterial,
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

/// C++ `NoiseType` (`fuzzy_skin_noise_type`). BBL default is `classic`.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
pub enum FuzzySkinNoiseType {
    #[default]
    Classic,
    Perlin,
    Billow,
    RidgedMulti,
    Voronoi,
}

impl FuzzySkinNoiseType {
    pub fn from_name(name: &str) -> Option<Self> {
        Some(match name.to_ascii_lowercase().as_str() {
            "classic" | "uniform" => Self::Classic,
            "perlin" => Self::Perlin,
            "billow" => Self::Billow,
            "ridgedmulti" | "ridged_multi" | "ridged-multi" => Self::RidgedMulti,
            "voronoi" | "cellular" => Self::Voronoi,
            _ => return None,
        })
    }

    pub fn as_str(self) -> &'static str {
        match self {
            Self::Classic => "classic",
            Self::Perlin => "perlin",
            Self::Billow => "billow",
            Self::RidgedMulti => "ridgedmulti",
            Self::Voronoi => "voronoi",
        }
    }
}

/// C++ `FuzzySkinMode` (`fuzzy_skin_mode`). Classic walls only displace.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
pub enum FuzzySkinMode {
    #[default]
    Displacement,
    Extrusion,
    Combined,
}

impl FuzzySkinMode {
    pub fn from_name(name: &str) -> Option<Self> {
        Some(match name.to_ascii_lowercase().as_str() {
            "displacement" | "displace" => Self::Displacement,
            "extrusion" | "width" => Self::Extrusion,
            "combined" | "both" => Self::Combined,
            _ => return None,
        })
    }

    pub fn as_str(self) -> &'static str {
        match self {
            Self::Displacement => "displacement",
            Self::Extrusion => "extrusion",
            Self::Combined => "combined",
        }
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

/// C++ `reduce_infill_retraction_mode`. BBL default is Auto.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
pub enum ReduceInfillRetractionMode {
    Disabled,
    #[default]
    Auto,
    Enabled,
}

impl ReduceInfillRetractionMode {
    pub fn from_name(name: &str) -> Option<Self> {
        Some(match name.to_ascii_lowercase().as_str() {
            "disabled" | "off" | "0" | "false" => Self::Disabled,
            "auto" | "1" => Self::Auto,
            "enabled" | "on" | "2" | "true" => Self::Enabled,
            _ => return None,
        })
    }

    pub fn as_str(self) -> &'static str {
        match self {
            Self::Disabled => "Disabled",
            Self::Auto => "Auto",
            Self::Enabled => "Enabled",
        }
    }
}

/// C++ `filament_metal_stickiness`. None (untested) behaves like Low.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
pub enum FilamentMetalStickiness {
    #[default]
    None,
    Low,
    Medium,
    High,
}

impl FilamentMetalStickiness {
    pub fn from_name(name: &str) -> Option<Self> {
        Some(match name.to_ascii_lowercase().as_str() {
            "none" | "untested" => Self::None,
            "low" => Self::Low,
            "medium" => Self::Medium,
            "high" => Self::High,
            _ => return None,
        })
    }

    pub fn as_str(self) -> &'static str {
        match self {
            Self::None => "None",
            Self::Low => "Low",
            Self::Medium => "Medium",
            Self::High => "High",
        }
    }

    /// C++ Auto mode: None and Low skip infill retraction.
    pub fn is_low_for_infill_retract(self) -> bool {
        matches!(self, Self::None | Self::Low)
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

/// C++ `gcode_flavor`. BBL profiles use Marlin (legacy).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
pub enum GCodeFlavor {
    #[default]
    Marlin,
    Klipper,
}

impl GCodeFlavor {
    pub fn from_name(name: &str) -> Option<Self> {
        Some(match name.to_ascii_lowercase().as_str() {
            "marlin" | "marlinlegacy" | "marlin(legacy)" | "marlin2" | "marlinfirmware" => {
                Self::Marlin
            }
            "klipper" => Self::Klipper,
            _ => return None,
        })
    }

    pub fn as_str(self) -> &'static str {
        match self {
            Self::Marlin => "marlin",
            Self::Klipper => "klipper",
        }
    }
}

/// C++ `EnsureVerticalThicknessLevel` (`ensure_vertical_shell_thickness`).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
pub enum EnsureVerticalShellThickness {
    Disabled,
    Partial,
    #[default]
    Enabled,
}

impl EnsureVerticalShellThickness {
    pub fn from_name(name: &str) -> Option<Self> {
        Some(match name.to_ascii_lowercase().as_str() {
            "disabled" | "0" | "false" | "off" | "none" => Self::Disabled,
            "partial" | "1" => Self::Partial,
            "enabled" | "2" | "true" | "on" => Self::Enabled,
            _ => return None,
        })
    }

    pub fn as_str(self) -> &'static str {
        match self {
            Self::Disabled => "disabled",
            Self::Partial => "partial",
            Self::Enabled => "enabled",
        }
    }
}

/// C++ `machine_max_*` used by `GCode::print_machine_envelope`.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct MachineLimits {
    pub acceleration_x_mm_s2: f64,
    pub acceleration_y_mm_s2: f64,
    pub acceleration_z_mm_s2: f64,
    pub acceleration_e_mm_s2: f64,
    pub speed_x_mm_s: f64,
    pub speed_y_mm_s: f64,
    pub speed_z_mm_s: f64,
    pub speed_e_mm_s: f64,
    pub acceleration_extruding_mm_s2: f64,
    pub acceleration_travel_mm_s2: f64,
}

impl Default for MachineLimits {
    fn default() -> Self {
        Self {
            acceleration_x_mm_s2: 1000.0,
            acceleration_y_mm_s2: 1000.0,
            acceleration_z_mm_s2: 500.0,
            acceleration_e_mm_s2: 5000.0,
            speed_x_mm_s: 500.0,
            speed_y_mm_s: 500.0,
            speed_z_mm_s: 12.0,
            speed_e_mm_s: 120.0,
            acceleration_extruding_mm_s2: 1500.0,
            acceleration_travel_mm_s2: 1500.0,
        }
    }
}

/// C++ first- vs later-layer bed temperatures for one plate type.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct PlateBedTemps {
    pub later_c: u16,
    pub initial_c: u16,
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
    /// C++ `initial_layer_line_width`. 0 keeps the role width (or `line_width`).
    pub initial_layer_line_width_mm: f64,
    /// C++ `initial_layer_infill_line_width`. First-layer sparse/solid/top fill
    /// width. 0 follows [`Self::initial_layer_line_width_mm`]. BBL `"0.5"`.
    pub initial_layer_infill_line_width_mm: f64,
    /// C++ `outer_wall_line_width`. 0 falls back to `line_width`.
    pub outer_wall_line_width_mm: f64,
    /// C++ `inner_wall_line_width`. 0 falls back to `line_width`.
    pub inner_wall_line_width_mm: f64,
    /// C++ `sparse_infill_line_width`. 0 falls back to `line_width`.
    pub sparse_infill_line_width_mm: f64,
    /// C++ `internal_solid_infill_line_width`. 0 falls back to `line_width`.
    pub internal_solid_infill_line_width_mm: f64,
    /// C++ `top_surface_line_width`. 0 falls back to `line_width`.
    pub top_surface_line_width_mm: f64,
    /// C++ `support_line_width`. 0 falls back to `line_width`.
    pub support_line_width_mm: f64,
    pub wall_loops: u32,
    /// C++ `alternate_extra_wall` (PrintRegionConfig, default false). Extra
    /// wall on odd object layers (`layer_id % 2 == 1`), skipped in spiral vase.
    pub alternate_extra_wall: bool,
    /// C++ `top_one_wall_type` (BBL default is `all top`).
    pub top_one_wall: TopOneWallType,
    /// C++ `only_one_wall_first_layer`. Drop inner walls on object layer 0.
    pub only_one_wall_first_layer: bool,
    pub infill_density: f64,
    pub infill_pattern: InfillPattern,
    /// C++ `fill_multiline` (1–5). Extra parallel copies of sparse infill.
    pub fill_multiline: u32,
    /// C++ `infill_combination`. Intersect sparse fill across layers that fit
    /// under the nozzle diameter and print the overlap on the uppermost layer.
    /// BBL `"0"`.
    pub infill_combination: bool,
    /// C++ `infill_direction` (degrees). Sparse/solid scanlines, gyroid, and honeycomb rotate by this.
    pub infill_direction_deg: f64,
    /// C++ `sparse_infill_lattice_angle_1`. BBL `-45`.
    pub sparse_infill_lattice_angle_1_deg: f64,
    /// C++ `sparse_infill_lattice_angle_2`. BBL `45`.
    pub sparse_infill_lattice_angle_2_deg: f64,
    /// C++ `sparse_infill_anchor` (mm or percent of sparse width).
    pub sparse_infill_anchor: f64,
    pub sparse_infill_anchor_is_percent: bool,
    /// C++ `sparse_infill_anchor_max`.
    pub sparse_infill_anchor_max: f64,
    pub sparse_infill_anchor_max_is_percent: bool,
    /// C++ `embedding_wall_into_infill`.
    pub embedding_wall_into_infill: bool,
    /// C++ `bridge_angle` (degrees). `0` keeps auto / infill direction; `> 0`
    /// forces that angle on bridged bottoms (`LayerRegion.cpp` `custom_angle > 0`).
    pub bridge_angle_deg: f64,
    /// C++ `symmetric_infill_y_axis`. Mirror fill about the object X-center so
    /// left/right twins share a texture. BBL `"0"`.
    pub symmetric_infill_y_axis: bool,
    /// C++ `minimum_sparse_infill_area` (mm²). Sparse islands at or below this
    /// become internal solid. 0 disables.
    pub minimum_sparse_infill_area_mm2: f64,
    /// C++ `infill_wall_overlap` as a fraction of line width (BBL default 15%).
    pub infill_wall_overlap: f64,
    pub seam: SeamPosition,
    /// C++ `seam_placement_away_from_overhangs`. Aligned / rear / nearest
    /// seams skip vertices that are not over the layer below. BBL `"0"`.
    pub seam_placement_away_from_overhangs: bool,
    /// C++ `seam_gap` as a fraction of nozzle diameter (default 15%).
    pub seam_gap: f64,
    /// C++ `seam_slope_type`. BBL `"none"`.
    pub seam_slope_type: SeamScarfType,
    /// C++ `filament_scarf_seam_type`. Used unless `override_filament_scarf_seam_setting`.
    pub filament_scarf_seam_type: SeamScarfType,
    /// C++ `override_filament_scarf_seam_setting`. BBL `"0"`.
    pub override_filament_scarf_seam_setting: bool,
    /// C++ `seam_slope_start_height` (mm or percent of layer height). Default 10%.
    pub seam_slope_start_height: f64,
    pub seam_slope_start_height_is_percent: bool,
    /// C++ `seam_slope_gap` (mm or percent of nozzle). BBL `"0"`.
    pub seam_slope_gap: f64,
    pub seam_slope_gap_is_percent: bool,
    /// C++ `seam_slope_min_length` (mm). 0 disables the scarf.
    pub seam_slope_min_length_mm: f64,
    /// C++ `seam_slope_entire_loop`.
    pub seam_slope_entire_loop: bool,
    /// C++ `seam_slope_steps`. Minimum segments along the ramp.
    pub seam_slope_steps: u32,
    /// C++ `seam_slope_inner_walls`. Scarf inner walls when the type is on.
    pub seam_slope_inner_walls: bool,
    /// C++ `seam_slope_conditional`. Default true; cube corners skip scarf.
    pub seam_slope_conditional: bool,
    /// C++ `scarf_angle_threshold` (degrees). BBL `155`.
    pub scarf_angle_threshold_deg: i32,
    /// C++ `apply_scarf_seam_on_circles`.
    pub apply_scarf_seam_on_circles: bool,
    /// C++ `filament_scarf_height` (mm or percent of layer height).
    pub filament_scarf_height: f64,
    pub filament_scarf_height_is_percent: bool,
    /// C++ `filament_scarf_gap` (mm or percent of nozzle).
    pub filament_scarf_gap: f64,
    pub filament_scarf_gap_is_percent: bool,
    /// C++ `filament_scarf_length` (mm).
    pub filament_scarf_length_mm: f64,
    pub wall_generator: WallGenerator,
    /// C++ `detect_thin_wall`. Open centerlines for islands that cannot hold
    /// two line widths. BBL `"0"`.
    pub detect_thin_wall: bool,
    /// C++ `wall_sequence`. BBL `wall_infill_order` remaps onto this.
    pub wall_sequence: WallSequence,
    /// C++ `precise_outer_wall`. BBL / PrintConfig default is false. Classic
    /// walls already inset by width (the InnerOuter ON path); this stores the
    /// key without flipping default gaps onto `Flow::spacing`.
    pub precise_outer_wall: bool,
    /// C++ `is_infill_first`. Print infill before walls except on G-code layer 0.
    pub is_infill_first: bool,
    /// C++ `min_feature_size` as a fraction of nozzle diameter (default 25%).
    pub min_feature_size: f64,
    /// C++ `min_bead_width` as a fraction of nozzle diameter (default 85%).
    pub min_bead_width: f64,
    /// C++ `wall_transition_length` as a fraction of nozzle (default 100%).
    pub wall_transition_length: f64,
    /// C++ `wall_transition_filter_deviation` as a fraction of nozzle (25%).
    pub wall_transition_filter_deviation: f64,
    /// C++ `wall_transition_angle` (degrees). Default 10.
    pub wall_transition_angle_deg: f64,
    /// C++ `wall_distribution_count`. Default 1.
    pub wall_distribution_count: u32,
    /// C++ `top_area_threshold` as a fraction of perimeter width (200%).
    pub top_area_threshold: f64,
    /// C++ `enable_circle_compensation`. BBL `"0"`.
    pub enable_circle_compensation: bool,
    /// C++ `fuzzy_skin` (BBL default is `none`).
    pub fuzzy_skin: FuzzySkinType,
    pub fuzzy_skin_thickness_mm: f64,
    pub fuzzy_skin_point_distance_mm: f64,
    /// Apply jitter on object layer 0 (`fuzzy_skin_first_layer`).
    pub fuzzy_skin_first_layer: bool,
    /// C++ `fuzzy_skin_noise_type`. BBL `"classic"`.
    pub fuzzy_skin_noise_type: FuzzySkinNoiseType,
    /// C++ `fuzzy_skin_mode`. Classic walls only displace; extrusion/combined are Arachne.
    pub fuzzy_skin_mode: FuzzySkinMode,
    /// C++ `fuzzy_skin_scale` (mm). Frequency is `1 / scale`.
    pub fuzzy_skin_scale: f64,
    /// C++ `fuzzy_skin_octaves`.
    pub fuzzy_skin_octaves: u32,
    /// C++ `fuzzy_skin_persistence`.
    pub fuzzy_skin_persistence: f64,
    pub nozzle_diameter_mm: f64,
    /// C++ `nozzle_diameter` (one entry per logical nozzle).
    pub nozzle_diameters_mm: Vec<f64>,
    pub filament_diameter_mm: f64,
    /// C++ `filament_flow_ratio`. Generic PLA @ H2C 0.4 is 0.99.
    pub flow_ratio: f64,
    /// C++ `top_solid_infill_flow_ratio` (first nozzle). Applied in G-code only.
    pub top_solid_infill_flow_ratio: f64,
    /// C++ `initial_layer_flow_ratio`. Applied in G-code only; top solid wins.
    pub initial_layer_flow_ratio: f64,
    /// C++ `print_flow_ratio` (PrintRegionConfig / calib). Multiplies every
    /// path in G-code before top-solid / first-layer ratios. Default 1; BBL
    /// omits the key.
    pub print_flow_ratio: f64,
    /// C++ `nozzle_temperature` (later layers).
    pub temperature_c: u16,
    /// C++ `nozzle_temperature_initial_layer`.
    pub temperature_initial_layer_c: u16,
    /// Later-layer bed temp for `curr_bed_type` (C++ `get_bed_temperature`).
    pub bed_temperature_c: u16,
    /// First-layer bed temp for `curr_bed_type`.
    pub bed_temperature_initial_layer_c: u16,
    pub print_speed_mm_s: f64,
    /// C++ `inner_wall_speed`. Defaults to the outer wall speed.
    pub inner_wall_speed_mm_s: f64,
    /// C++ `initial_layer_speed` (first printed layer walls and most features).
    pub first_layer_speed_mm_s: f64,
    /// C++ `initial_layer_infill_speed`.
    pub first_layer_infill_speed_mm_s: f64,
    pub infill_speed_mm_s: f64,
    /// C++ `gap_infill_speed`. 0 disables gap fill.
    pub gap_infill_speed_mm_s: f64,
    /// C++ `filter_out_gap_fill` (mm). Drop shorter gap polylines.
    pub filter_out_gap_fill_mm: f64,
    pub travel_speed_mm_s: f64,
    pub support_speed_mm_s: f64,
    /// C++ `support_interface_speed`.
    pub support_interface_speed_mm_s: f64,
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
    /// C++ `override_process_overhang_speed`.
    pub override_process_overhang_speed: bool,
    /// C++ `filament_enable_overhang_speed`. Default true.
    pub filament_enable_overhang_speed: bool,
    pub filament_overhang_1_4_speed_mm_s: f64,
    pub filament_overhang_2_4_speed_mm_s: f64,
    pub filament_overhang_3_4_speed_mm_s: f64,
    pub filament_overhang_4_4_speed_mm_s: f64,
    pub filament_overhang_speed_mm_s: f64,
    pub filament_bridge_speed_mm_s: f64,
    /// C++ `enable_height_slowdown`.
    pub enable_height_slowdown: bool,
    pub slowdown_start_height_mm: f64,
    pub slowdown_start_speed_mm_s: f64,
    pub slowdown_start_acc_mm_s2: f64,
    pub slowdown_end_height_mm: f64,
    pub slowdown_end_speed_mm_s: f64,
    pub slowdown_end_acc_mm_s2: f64,
    /// C++ `bridge_speed`.
    pub bridge_speed_mm_s: f64,
    /// C++ `bridge_flow` (PrintConfig default 1). Scales bridge extrusion volume.
    pub bridge_flow: f64,
    /// C++ `thick_bridges` (PrintObjectConfig, default false). Circular
    /// `sqrt(bridge_flow) * nozzle` extrusion instead of a scaled rectangle.
    pub thick_bridges: bool,
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
    /// C++ `skirt_height` (layers). 0 disables the skirt even when loops > 0
    /// unless [`Self::draft_shield`] is on.
    pub skirt_height: u32,
    /// C++ `draft_shield`. BBL `disabled`.
    pub draft_shield: DraftShield,
    /// C++ `ooze_prevention` (PrintConfig, default false). With more than one
    /// extruder this makes the skirt as tall as the object.
    pub ooze_prevention: bool,
    /// Gap between the outermost brim (or the object) and the innermost skirt loop.
    pub skirt_distance_mm: f64,
    /// Outer brim width on layer 0 (0 disables). Ignored when [`Self::raft_layers`] > 0
    /// or [`Self::brim_type`] has no outer brim.
    pub brim_width_mm: f64,
    /// C++ `brim_type`. BBL omits the key (`auto_brim`).
    pub brim_type: BrimType,
    /// C++ `brim_object_gap` (mm). Space between the object and the innermost brim loop.
    pub brim_object_gap_mm: f64,
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
    /// C++ `support_filament` (1-based). 0 keeps the current nozzle.
    pub support_filament: i32,
    /// C++ `support_interface_filament` (1-based). 0 keeps the current nozzle.
    pub support_interface_filament: i32,
    /// C++ `support_on_build_plate_only`. Don't rest support on the model.
    pub support_on_build_plate_only: bool,
    /// C++ `max_bridge_length` (mm). 0 supports every bridge (BBL JSON `"0"`).
    /// When `> 0`, two-sided overhangs whose bbox is shorter than this in both
    /// axes are not supported (`PrintObject::remove_bridges_from_contacts`).
    pub max_bridge_length_mm: f64,
    /// C++ `bridge_no_support`. Skip support under every two-sided bridge,
    /// regardless of span. BBL `"0"`.
    pub bridge_no_support: bool,
    /// C++ `support_remove_small_overhang` (PrintObjectConfig, default true).
    /// Drop dust-sized overhangs; cantilevers farther than 3 mm stay.
    pub support_remove_small_overhang: bool,
    /// C++ `support_critical_regions_only`. Tree auto keeps cantilevers
    /// (and painted enforcers); other overhangs are dropped. BBL absent / false.
    pub support_critical_regions_only: bool,
    /// C++ `support_type`. Default is classic columns; BBL profiles use tree.
    pub support_type: SupportType,
    /// Maximum overhang angle from vertical that does not need support (degrees).
    pub support_threshold_angle_deg: f64,
    pub support_density: f64,
    pub support_xy_distance_mm: f64,
    /// C++ `support_object_first_layer_gap` (mm). XY gap on object layer 0.
    /// C++ default 0.2; BBL omits the key.
    pub support_object_first_layer_gap_mm: f64,
    /// C++ `support_top_z_distance` (mm). Air gap under the overhang. BBL `"0.2"`.
    pub support_top_z_distance_mm: f64,
    /// C++ `support_bottom_z_distance` (mm). Air gap above in-model landings.
    /// `<= 0` copies the top gap. BBL `"0.2"`.
    pub support_bottom_z_distance_mm: f64,
    pub support_interface_layers: u32,
    /// C++ `support_interface_bottom_layers`. `-1` means same as top; default 0
    /// (no in-model floors). BBL `"2"`.
    pub support_interface_bottom_layers: i32,
    /// C++ `support_bottom_interface_spacing` (mm). BBL omits; default 0.5.
    pub support_bottom_interface_spacing_mm: f64,
    /// C++ `support_interface_loop_pattern`. Cover the top contact with loops
    /// instead of hatch. BBL `"0"`.
    pub support_interface_loop_pattern: bool,
    /// C++ `support_base_pattern`. BBL `"default"` → classic rectilinear.
    pub support_base_pattern: SupportBasePattern,
    /// C++ `support_base_pattern_spacing` (mm). BBL `"2.5"`. Default hatch
    /// still uses [`Self::support_density`]; a non-default value uses the C++
    /// `spacing + flow.spacing()` formula.
    pub support_base_pattern_spacing_mm: f64,
    /// C++ `support_interface_pattern`. BBL `"auto"` stays 0° rectilinear hatch.
    pub support_interface_pattern: SupportInterfacePattern,
    /// C++ `support_interface_spacing` (mm). BBL `"0.5"`. Default hatch stays
    /// `width × 1.1`; a non-default value uses the C++ `spacing + flow.spacing()`
    /// formula. Zero is a solid interface (required for support ironing).
    pub support_interface_spacing_mm: f64,
    /// C++ `support_angle` (degrees). Rotate base/interface hatch. Default 0.
    pub support_angle_deg: f64,
    /// C++ `enable_support_ironing`. Iron a solid support interface. BBL `"0"`.
    pub enable_support_ironing: bool,
    /// C++ `support_ironing_pattern` (`zig-zag` / concentric).
    pub support_ironing_pattern: IroningPattern,
    /// C++ `support_ironing_spacing` (mm). BBL `"0.15"`.
    pub support_ironing_spacing_mm: f64,
    /// C++ `support_ironing_inset` (mm). BBL `"0"`.
    pub support_ironing_inset_mm: f64,
    /// C++ `support_ironing_direction` (degrees). Rotate support-interface
    /// ironing independently of [`Self::support_angle_deg`]. BBL `"0"`.
    pub support_ironing_direction_deg: f64,
    /// C++ `support_ironing_flow` as a fraction of layer height. Default 10%;
    /// BBL `"10%"`.
    pub support_ironing_flow: f64,
    /// C++ `support_ironing_speed` (mm/s). Default 20; BBL `"30"`.
    pub support_ironing_speed_mm_s: f64,
    /// C++ `support_expansion` (mm). Grow (+) or shrink (−) the contact
    /// footprint. BBL `"0"`.
    pub support_expansion_mm: f64,
    /// Max XY lean per layer (`tree_support_branch_angle`).
    pub tree_branch_angle_deg: f64,
    /// Disk diameter at each tree node (`tree_support_branch_diameter`).
    pub tree_branch_diameter_mm: f64,
    /// C++ `tree_support_branch_diameter_angle` (degrees). Trunks thicken
    /// toward the plate by `tan(angle)`. Default 5; BBL omits the key.
    pub tree_branch_diameter_angle_deg: f64,
    /// C++ `tree_support_branch_distance` (mm). Spacing of tree contact samples.
    /// C++ default 5; BBL omits the key.
    pub tree_branch_distance_mm: f64,
    /// C++ `tree_support_wall_count` in `[-1, 2]`. BBL `"-1"` auto keeps the
    /// current disk outlines; `1`/`2` add sheath loops; `0` is infill-only.
    pub tree_support_wall_count: i32,
    /// C++ `interface_shells`. When on, top/bottom shells form against the
    /// same region instead of every material on the neighboring layer. BBL `"0"`.
    pub interface_shells: bool,
    /// Solid layers at the bottom of the part (0 disables).
    pub bottom_shell_layers: u32,
    /// Solid layers at the top of the part (0 disables).
    pub top_shell_layers: u32,
    /// C++ `top_shell_thickness` (mm). 0 means layer count only.
    pub top_shell_thickness_mm: f64,
    /// C++ `bottom_shell_thickness` (mm). 0 means layer count only.
    pub bottom_shell_thickness_mm: f64,
    /// C++ `ensure_vertical_shell_thickness`.
    pub ensure_vertical_shell_thickness: EnsureVerticalShellThickness,
    /// C++ `top_surface_pattern` (BBL 0.20 is `monotonicline`).
    pub top_surface_pattern: SurfacePattern,
    /// C++ `bottom_surface_pattern` (BBL common is `monotonic`).
    pub bottom_surface_pattern: SurfacePattern,
    /// C++ `internal_solid_infill_pattern`. Wide internal solid; narrow islands
    /// stay concentric when `detect_narrow_internal_solid_infill` is on.
    /// C++ / BBL default is rectilinear.
    pub internal_solid_infill_pattern: SurfacePattern,
    /// C++ `sub_top_surface_pattern`. Solid under a visible top (`stSubTop`).
    /// C++ default is monotonic; BBL omits the key.
    pub sub_top_surface_pattern: SurfacePattern,
    /// C++ `monotonic_travel_into_wall` as a fraction of line width. Extends
    /// MonotonicLine travel into the walls. Default 0; BBL common `"0.0"`, dual
    /// `"45.0"`.
    pub monotonic_travel_into_wall: f64,
    /// C++ `top_surface_density` as a 0–1 fraction (PrintConfig default 100%).
    pub top_surface_density: f64,
    /// C++ `bottom_surface_density` as a 0–1 fraction (PrintConfig default 100%).
    pub bottom_surface_density: f64,
    /// C++ `detect_narrow_internal_solid_infill`.
    pub detect_narrow_internal_solid_infill: bool,
    /// C++ `detect_floating_vertical_shell` (per-segment slowdown).
    pub detect_floating_vertical_shell: bool,
    /// C++ `vertical_shell_speed` raw value (mm/s, or percent of internal solid).
    pub vertical_shell_speed: f64,
    /// When set, [`Self::vertical_shell_speed`] is a percent of solid infill speed.
    pub vertical_shell_speed_is_percent: bool,
    pub solid_infill_speed_mm_s: f64,
    /// C++ `wall_filament` (1-based). 0 means default / inherit.
    pub wall_filament: i32,
    /// C++ `sparse_infill_filament` (1-based). 0 means default / inherit.
    pub sparse_infill_filament: i32,
    /// C++ `solid_infill_filament` (1-based). 0 means default / inherit.
    pub solid_infill_filament: i32,
    pub ironing_type: IroningType,
    pub ironing_pattern: IroningPattern,
    /// Fraction of normal layer height (C++ `ironing_flow` percent).
    pub ironing_flow: f64,
    pub ironing_spacing_mm: f64,
    /// Inset from the ironed contour. `0` means half the nozzle diameter.
    pub ironing_inset_mm: f64,
    /// C++ `ironing_direction` (degrees). Combined with infill direction
    /// mod 180 (`Fill.cpp`). Default 45; BBL omits the key.
    pub ironing_direction_deg: f64,
    pub ironing_speed_mm_s: f64,
    /// C++ `default_acceleration` (mm/s²). Used by the G-code time estimator.
    pub default_acceleration_mm_s2: f64,
    /// C++ `outer_wall_acceleration`. 0 means use default.
    pub outer_wall_acceleration_mm_s2: f64,
    /// C++ `inner_wall_acceleration`. 0 means use default.
    pub inner_wall_acceleration_mm_s2: f64,
    /// C++ `initial_layer_acceleration`. 0 means use default.
    pub initial_layer_acceleration_mm_s2: f64,
    /// C++ `top_surface_acceleration`. 0 means use default.
    pub top_surface_acceleration_mm_s2: f64,
    /// C++ `sparse_infill_acceleration` (mm/s² or percent of default).
    pub sparse_infill_acceleration: f64,
    pub sparse_infill_acceleration_is_percent: bool,
    /// C++ `travel_acceleration` (mm/s²).
    pub travel_acceleration_mm_s2: f64,
    /// C++ `initial_layer_travel_acceleration`. 0 means use travel acceleration.
    pub initial_layer_travel_acceleration_mm_s2: f64,
    /// C++ `travel_short_distance_acceleration`. 0 disables the VFA short-hop.
    pub travel_short_distance_acceleration_mm_s2: f64,
    /// C++ `machine_max_acceleration_retracting`. 0 means 1500.
    pub retract_acceleration_mm_s2: f64,
    /// C++ `machine_max_*` axis limits for Marlin M201/M203/M204.
    pub machine_limits: MachineLimits,
    /// C++ `filament_density` (g/cm³). Generic PLA is 1.24.
    pub filament_density_g_cm3: f64,
    /// C++ `fan_min_speed` (percent).
    pub fan_min_speed: u32,
    /// C++ `fan_max_speed` (percent).
    pub fan_max_speed: u32,
    /// C++ `enable_overhang_bridge_fan`.
    pub enable_overhang_bridge_fan: bool,
    /// C++ `overhang_fan_speed` (percent).
    pub overhang_fan_speed: u32,
    /// C++ `overhang_fan_threshold`.
    pub overhang_fan_threshold: OverhangFanThreshold,
    /// C++ `ironing_fan_speed` (percent). `-1` disables.
    pub ironing_fan_speed: i32,
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
    /// C++ `slow_down_for_layer_cooling`.
    pub slow_down_for_layer_cooling: bool,
    /// C++ `no_slow_down_for_cooling_on_outwalls`.
    pub no_slow_down_for_cooling_on_outwalls: bool,
    /// C++ `cooling_slowdown_logic`. BBL `uniform_cooling`.
    pub cooling_slowdown_logic: CoolingSlowdownLogic,
    /// C++ `cooling_perimeter_transition_distance` (mm).
    pub cooling_perimeter_transition_distance_mm: f64,
    /// C++ `slow_down_min_speed` (mm/s). 0 means no floor.
    pub slow_down_min_speed_mm_s: f64,
    /// C++ `reduce_fan_stop_start_freq` (keep fan at least at min speed).
    pub reduce_fan_stop_start_freq: bool,
    /// C++ `auxiliary_fan` (machine has a side/aux cooling fan, `M106 P2`).
    pub auxiliary_fan: bool,
    /// C++ `additional_cooling_fan_speed` (percent).
    pub additional_cooling_fan_speed: u32,
    /// C++ `close_additional_fan_first_x_layers`.
    pub close_additional_fan_first_x_layers: u32,
    /// C++ `additional_fan_full_speed_layer`. 0 disables the aux ramp.
    pub additional_fan_full_speed_layer: u32,
    /// C++ `first_x_layer_fan_speed` (percent). Aux fan on closed first layers.
    pub first_x_layer_fan_speed: u32,
    /// C++ `pre_start_fan_time` (seconds). Spin the overhang fan up early.
    pub pre_start_fan_time_s: f64,
    /// C++ `support_air_filtration` (machine has an exhaust fan, `M106 P3`).
    pub support_air_filtration: bool,
    /// C++ `activate_air_filtration`.
    pub activate_air_filtration: bool,
    /// C++ `during_print_exhaust_fan_speed` (percent).
    pub during_print_exhaust_fan_speed: u32,
    /// C++ `complete_print_exhaust_fan_speed` (percent).
    pub complete_print_exhaust_fan_speed: u32,
    /// C++ `filament_max_volumetric_speed` (mm³/s). 0 disables the cap.
    pub filament_max_volumetric_speed_mm3_s: f64,
    /// C++ `retraction_length` (mm). 0 disables retraction.
    pub retraction_length_mm: f64,
    /// C++ `retraction_speed` (mm/s).
    pub retraction_speed_mm_s: f64,
    /// C++ `deretraction_speed` (mm/s). 0 means use retraction speed.
    pub deretraction_speed_mm_s: f64,
    /// C++ `retraction_minimum_travel` (mm).
    pub retraction_minimum_travel_mm: f64,
    /// C++ `reduce_infill_retraction_mode`.
    pub reduce_infill_retraction_mode: ReduceInfillRetractionMode,
    /// C++ `reduce_crossing_wall`. Detour travels that would cross walls. BBL `"0"`.
    pub reduce_crossing_wall: bool,
    /// C++ `max_travel_detour_distance` (mm or percent). 0 disables the cap.
    pub max_travel_detour_distance: f64,
    pub max_travel_detour_is_percent: bool,
    /// C++ `avoid_crossing_wall_includes_support`. BBL `"0"`.
    pub avoid_crossing_wall_includes_support: bool,
    /// C++ `filament_metal_stickiness`. Auto mode treats None like Low.
    pub filament_metal_stickiness: FilamentMetalStickiness,
    /// C++ `retract_when_changing_layer`.
    pub retract_when_changing_layer: bool,
    /// C++ `wipe` (wipe along the last path while retracting).
    pub wipe: bool,
    /// C++ `wipe_distance` (mm).
    pub wipe_distance_mm: f64,
    /// C++ `retract_before_wipe` as a 0–1 factor.
    pub retract_before_wipe: f64,
    /// C++ `wipe_speed` percent of travel (when not role-based).
    pub wipe_speed_percent: f64,
    /// C++ `role_base_wipe_speed`.
    pub role_base_wipe_speed: bool,
    /// C++ `retract_restart_extra` (mm extra on unretract).
    pub retract_restart_extra_mm: f64,
    /// C++ `z_hop` (mm). 0 disables the lift.
    pub z_hop_mm: f64,
    /// C++ `z_hop_types`.
    pub z_hop_type: ZHopType,
    /// C++ `retract_lift_above` (mm). Hop only at or above this Z.
    pub retract_lift_above_mm: f64,
    /// C++ `retract_lift_below` (mm). Hop only at or below this Z. 0 disables hop.
    pub retract_lift_below_mm: f64,
    /// C++ `travel_speed_z` (mm/s). 0 means use travel speed.
    pub travel_speed_z_mm_s: f64,
    /// Printable-area AABB. Invalid means C++ "no block" (any spiral I/J is allowed).
    pub bed_bbox_valid: bool,
    pub bed_min_x: f64,
    pub bed_min_y: f64,
    pub bed_max_x: f64,
    pub bed_max_y: f64,
    /// C++ `machine_max_jerk_x` / `_y` (mm/s). X1 Carbon default is 9.
    pub xy_jerk_mm_s: f64,
    /// C++ `machine_max_jerk_z` (mm/s). X1 Carbon default is 3.
    pub z_jerk_mm_s: f64,
    /// C++ `machine_max_jerk_e` (mm/s).
    pub e_jerk_mm_s: f64,
    /// C++ `gcode_flavor`.
    pub gcode_flavor: GCodeFlavor,
    /// C++ `layer_change_gcode`. Empty skips the custom block.
    pub layer_change_gcode: String,
    /// C++ `machine_end_gcode`. Empty keeps the generic Marlin footer.
    pub machine_end_gcode: String,
    /// C++ `filament_end_gcode` for the active filament.
    pub filament_end_gcode: String,
    /// C++ `long_retractions_when_cut` / placeholder `long_retraction_when_cut`.
    pub long_retraction_when_cut: bool,
    /// C++ `long_retractions_when_ec` / placeholder `long_retraction_when_ec`.
    pub long_retraction_when_ec: bool,
    /// C++ `retraction_distances_when_cut`.
    pub retraction_distance_when_cut: f64,
    /// C++ `retraction_distances_when_ec`.
    pub retraction_distance_when_ec: f64,
    /// C++ `machine_start_gcode`. Empty keeps the generic Marlin header.
    pub machine_start_gcode: String,
    /// C++ `filament_start_gcode` for the active filament.
    pub filament_start_gcode: String,
    /// C++ `filament_type`.
    pub filament_type: String,
    /// C++ `filament_vendor`.
    pub filament_vendor: String,
    /// C++ `chamber_temperatures` / placeholder `overall_chamber_temperature`.
    pub chamber_temperature_c: i32,
    /// C++ `temperature_vitrification`.
    pub temperature_vitrification_c: i32,
    /// C++ `cooling_filter_enabled`.
    pub cooling_filter_enabled: bool,
    /// C++ `curr_bed_type` display name (`Cool Plate`, `Textured PEI Plate`, …).
    pub curr_bed_type: String,
    /// C++ `cool_plate_temp` / `cool_plate_temp_initial_layer`.
    pub cool_plate: PlateBedTemps,
    /// C++ `eng_plate_temp` / `eng_plate_temp_initial_layer`.
    pub eng_plate: PlateBedTemps,
    /// C++ `hot_plate_temp` / `hot_plate_temp_initial_layer` (High Temp Plate).
    pub hot_plate: PlateBedTemps,
    /// C++ `textured_plate_temp` / `textured_plate_temp_initial_layer`.
    pub textured_plate: PlateBedTemps,
    /// C++ `supertack_plate_temp` / `supertack_plate_temp_initial_layer`.
    pub supertack_plate: PlateBedTemps,
    /// C++ `nozzle_temperature_range_high` (flush-temp fallback).
    pub nozzle_temperature_range_high: u16,
    /// C++ `time_lapse_gcode`. Empty skips per-layer insert.
    pub time_lapse_gcode: String,
    /// C++ `timelapse_type`: 0 traditional, 1 smooth.
    pub timelapse_type: u8,
    /// C++ `farthest_point_timelapse`.
    pub farthest_point_timelapse: bool,
    /// C++ `spiral_mode`.
    pub spiral_mode: bool,
    /// C++ `enable_arc_fitting`. Off keeps G1; BBL common default is on.
    pub enable_arc_fitting: bool,
    /// C++ `resolution` (mm). Arc-fit and path-simplify tolerance.
    pub resolution_mm: f64,
    /// C++ `enable_wrapping_detection`. Off skips the per-layer insert.
    pub enable_wrapping_detection: bool,
    /// C++ `wrapping_detection_gcode`. Empty skips even when wrapping is on.
    pub wrapping_detection_gcode: String,
    /// C++ `filament_map` (1-based extruder ids). Default is left nozzle.
    pub filament_map: Vec<i32>,
    /// C++ `physical_extruder_map` (logical filament id → physical nozzle).
    pub physical_extruder_map: Vec<i32>,
    /// C++ `scan_first_layer`: nozzle-cam inspect on the second layer.
    pub scan_first_layer: bool,
    /// C++ `printer_structure` (`corexy`, `i3`, …).
    pub printer_structure: String,
    /// C++ `print_sequence` (`by layer` / `by object`).
    pub print_sequence: String,
    /// C++ `skirt_per_object`. Default true; only sequential by-object uses it.
    pub skirt_per_object: bool,
    /// C++ `standby_temperature_delta`. BBL `-5`.
    pub standby_temperature_delta_c: i32,
    /// C++ `independent_support_layer_height`. Default true; invalid with wipe tower.
    pub independent_support_layer_height: bool,
    /// C++ `filament_shrink` percent (100 = no compensation).
    pub filament_shrink_percent: f64,
    /// C++ `printable_height` (placeholder `max_print_height`).
    pub printable_height_mm: f64,
    /// C++ `printable_area` polygon (mm). Empty falls back to the bed AABB.
    pub printable_area: Vec<(f64, f64)>,
    /// C++ `extruder_printable_area` per logical extruder (mm).
    pub extruder_printable_areas: Vec<Vec<(f64, f64)>>,
    /// C++ `bed_exclude_area` polygon (mm).
    pub bed_exclude_area: Vec<(f64, f64)>,
    /// C++ `extruder_clearance_max_radius` (mm). Timelapse keep-out.
    pub extruder_clearance_max_radius_mm: f64,
    /// C++ `enable_prime_tower`.
    pub enable_prime_tower: bool,
    /// C++ `wipe_tower_x` for the current plate (mm).
    pub wipe_tower_x_mm: f64,
    /// C++ `wipe_tower_y` for the current plate (mm).
    pub wipe_tower_y_mm: f64,
    /// C++ `prime_tower_width` (mm).
    pub prime_tower_width_mm: f64,
    /// C++ `prime_tower_brim_width` (mm). Negative means auto; keep-out treats it as 0.
    pub prime_tower_brim_width_mm: f64,
    /// C++ `prime_tower_max_speed` (mm/s). Default 90; BBL `"90"`.
    pub prime_tower_max_speed_mm_s: f64,
    /// C++ `wipe_tower_no_sparse_layers`. Skip the tower on layers with no
    /// toolchange (not smooth timelapse / wrapping). BBL `"0"`.
    pub wipe_tower_no_sparse_layers: bool,
    /// C++ `enable_tower_interface_features`. Off in common; H2C Standard `"1"`.
    pub enable_tower_interface_features: bool,
    /// C++ `prime_tower_lift_height`. BBL `-1` disables.
    pub prime_tower_lift_height_mm: f64,
    /// C++ `prime_tower_lift_speed`. Default 90 mm/s.
    pub prime_tower_lift_speed_mm_s: f64,
    /// C++ `prime_tower_enable_framework`.
    pub prime_tower_enable_framework: bool,
    /// C++ `prime_tower_flat_ironing`.
    pub prime_tower_flat_ironing: bool,
    /// C++ `flush_into_objects` / infill / support.
    pub flush_into_objects: bool,
    pub flush_into_infill: bool,
    pub flush_into_support: bool,
    /// C++ `flush_volumes_matrix` (n×n mm³). Empty uses 800 mm³ between tools.
    pub flush_volumes_mm3: Vec<f64>,
    /// C++ `filament_diameter.values.size()`.
    pub filament_count: usize,
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
            initial_layer_line_width_mm: 0.0,
            initial_layer_infill_line_width_mm: 0.0,
            outer_wall_line_width_mm: 0.0,
            inner_wall_line_width_mm: 0.0,
            sparse_infill_line_width_mm: 0.0,
            internal_solid_infill_line_width_mm: 0.0,
            top_surface_line_width_mm: 0.0,
            support_line_width_mm: 0.0,
            wall_loops: 2,
            alternate_extra_wall: false,
            top_one_wall: TopOneWallType::None,
            only_one_wall_first_layer: false,
            infill_density: 0.20,
            infill_pattern: InfillPattern::Gyroid,
            fill_multiline: 1,
            infill_combination: false,
            infill_direction_deg: 45.0,
            sparse_infill_lattice_angle_1_deg: -45.0,
            sparse_infill_lattice_angle_2_deg: 45.0,
            sparse_infill_anchor: 600.0,
            sparse_infill_anchor_is_percent: true,
            sparse_infill_anchor_max: 0.0,
            sparse_infill_anchor_max_is_percent: false,
            embedding_wall_into_infill: false,
            bridge_angle_deg: 0.0,
            symmetric_infill_y_axis: false,
            minimum_sparse_infill_area_mm2: 15.0,
            infill_wall_overlap: 0.15,
            seam: SeamPosition::Aligned,
            seam_placement_away_from_overhangs: false,
            seam_gap: 0.15,
            seam_slope_type: SeamScarfType::None,
            filament_scarf_seam_type: SeamScarfType::None,
            override_filament_scarf_seam_setting: false,
            seam_slope_start_height: 10.0,
            seam_slope_start_height_is_percent: true,
            seam_slope_gap: 0.0,
            seam_slope_gap_is_percent: false,
            seam_slope_min_length_mm: 10.0,
            seam_slope_entire_loop: false,
            seam_slope_steps: 10,
            seam_slope_inner_walls: true,
            seam_slope_conditional: true,
            scarf_angle_threshold_deg: 155,
            apply_scarf_seam_on_circles: false,
            filament_scarf_height: 10.0,
            filament_scarf_height_is_percent: true,
            filament_scarf_gap: 0.0,
            filament_scarf_gap_is_percent: true,
            filament_scarf_length_mm: 10.0,
            wall_generator: WallGenerator::Classic,
            detect_thin_wall: false,
            wall_sequence: WallSequence::InnerOuter,
            precise_outer_wall: false,
            is_infill_first: false,
            min_feature_size: 0.25,
            min_bead_width: 0.85,
            wall_transition_length: 1.0,
            wall_transition_filter_deviation: 0.25,
            wall_transition_angle_deg: 10.0,
            wall_distribution_count: 1,
            top_area_threshold: 2.0,
            enable_circle_compensation: false,
            fuzzy_skin: FuzzySkinType::None,
            fuzzy_skin_thickness_mm: 0.3,
            fuzzy_skin_point_distance_mm: 0.8,
            fuzzy_skin_first_layer: false,
            fuzzy_skin_noise_type: FuzzySkinNoiseType::Classic,
            fuzzy_skin_mode: FuzzySkinMode::Displacement,
            fuzzy_skin_scale: 1.0,
            fuzzy_skin_octaves: 4,
            fuzzy_skin_persistence: 0.5,
            nozzle_diameter_mm: 0.4,
            nozzle_diameters_mm: vec![0.4],
            filament_diameter_mm: 1.75,
            flow_ratio: 1.0,
            top_solid_infill_flow_ratio: 1.0,
            initial_layer_flow_ratio: 1.0,
            print_flow_ratio: 1.0,
            temperature_c: 220,
            temperature_initial_layer_c: 220,
            bed_temperature_c: 60,
            bed_temperature_initial_layer_c: 60,
            print_speed_mm_s: 50.0,
            inner_wall_speed_mm_s: 50.0,
            first_layer_speed_mm_s: 50.0,
            first_layer_infill_speed_mm_s: 80.0,
            infill_speed_mm_s: 80.0,
            gap_infill_speed_mm_s: 30.0,
            filter_out_gap_fill_mm: 0.0,
            travel_speed_mm_s: 120.0,
            support_speed_mm_s: 80.0,
            support_interface_speed_mm_s: 80.0,
            detect_overhang_wall: true,
            enable_overhang_speed: true,
            overhang_1_4_speed_mm_s: 0.0,
            overhang_2_4_speed_mm_s: 50.0,
            overhang_3_4_speed_mm_s: 30.0,
            overhang_4_4_speed_mm_s: 10.0,
            overhang_speed_mm_s: 10.0,
            override_process_overhang_speed: false,
            filament_enable_overhang_speed: true,
            filament_overhang_1_4_speed_mm_s: 0.0,
            filament_overhang_2_4_speed_mm_s: 50.0,
            filament_overhang_3_4_speed_mm_s: 30.0,
            filament_overhang_4_4_speed_mm_s: 10.0,
            filament_overhang_speed_mm_s: 10.0,
            filament_bridge_speed_mm_s: 25.0,
            enable_height_slowdown: false,
            slowdown_start_height_mm: 0.0,
            slowdown_start_speed_mm_s: 0.0,
            slowdown_start_acc_mm_s2: 0.0,
            slowdown_end_height_mm: 0.0,
            slowdown_end_speed_mm_s: 0.0,
            slowdown_end_acc_mm_s2: 0.0,
            bridge_speed_mm_s: 25.0,
            bridge_flow: 1.0,
            thick_bridges: false,
            top_surface_speed_mm_s: 50.0,
            small_perimeter_speed: 50.0,
            small_perimeter_speed_is_percent: true,
            small_perimeter_threshold_mm: 0.0,
            skirt_loops: 2,
            skirt_height: 1,
            draft_shield: DraftShield::Disabled,
            ooze_prevention: false,
            skirt_distance_mm: 2.0,
            brim_width_mm: 0.0,
            brim_type: BrimType::AutoBrim,
            brim_object_gap_mm: 0.0,
            raft_layers: 0,
            raft_contact_distance_mm: 0.1,
            raft_expansion_mm: 1.5,
            raft_first_layer_expansion_mm: -1.0,
            raft_first_layer_density: 0.90,
            enable_support: false,
            support_filament: 0,
            support_interface_filament: 0,
            support_on_build_plate_only: false,
            max_bridge_length_mm: 0.0,
            bridge_no_support: false,
            support_remove_small_overhang: true,
            support_critical_regions_only: false,
            support_type: SupportType::Classic,
            support_threshold_angle_deg: 30.0,
            support_density: 0.15,
            support_xy_distance_mm: 0.35,
            support_object_first_layer_gap_mm: 0.2,
            support_top_z_distance_mm: 0.2,
            support_bottom_z_distance_mm: 0.2,
            support_interface_layers: 2,
            support_interface_bottom_layers: 0,
            support_bottom_interface_spacing_mm: 0.5,
            support_interface_loop_pattern: false,
            support_base_pattern: SupportBasePattern::Default,
            support_base_pattern_spacing_mm: 2.5,
            support_interface_pattern: SupportInterfacePattern::Auto,
            support_interface_spacing_mm: 0.5,
            support_angle_deg: 0.0,
            enable_support_ironing: false,
            support_ironing_pattern: IroningPattern::Rectilinear,
            support_ironing_spacing_mm: 0.15,
            support_ironing_inset_mm: 0.0,
            support_ironing_direction_deg: 0.0,
            support_ironing_flow: 0.10,
            support_ironing_speed_mm_s: 20.0,
            support_expansion_mm: 0.0,
            tree_branch_angle_deg: 45.0,
            tree_branch_diameter_mm: 2.0,
            tree_branch_diameter_angle_deg: 5.0,
            tree_branch_distance_mm: 5.0,
            tree_support_wall_count: -1,
            interface_shells: false,
            bottom_shell_layers: 3,
            top_shell_layers: 3,
            top_shell_thickness_mm: 0.0,
            bottom_shell_thickness_mm: 0.0,
            ensure_vertical_shell_thickness: EnsureVerticalShellThickness::Enabled,
            top_surface_pattern: SurfacePattern::Rectilinear,
            bottom_surface_pattern: SurfacePattern::Rectilinear,
            internal_solid_infill_pattern: SurfacePattern::Rectilinear,
            sub_top_surface_pattern: SurfacePattern::Monotonic,
            monotonic_travel_into_wall: 0.0,
            top_surface_density: 1.0,
            bottom_surface_density: 1.0,
            detect_narrow_internal_solid_infill: true,
            detect_floating_vertical_shell: true,
            vertical_shell_speed: 80.0,
            vertical_shell_speed_is_percent: true,
            solid_infill_speed_mm_s: 80.0,
            wall_filament: 0,
            sparse_infill_filament: 0,
            solid_infill_filament: 0,
            ironing_type: IroningType::NoIroning,
            ironing_pattern: IroningPattern::Rectilinear,
            ironing_flow: 0.10,
            ironing_spacing_mm: 0.15,
            ironing_inset_mm: 0.21,
            ironing_direction_deg: 45.0,
            ironing_speed_mm_s: 30.0,
            default_acceleration_mm_s2: 10000.0,
            outer_wall_acceleration_mm_s2: 0.0,
            inner_wall_acceleration_mm_s2: 0.0,
            initial_layer_acceleration_mm_s2: 0.0,
            top_surface_acceleration_mm_s2: 0.0,
            sparse_infill_acceleration: 0.0,
            sparse_infill_acceleration_is_percent: false,
            travel_acceleration_mm_s2: 10000.0,
            initial_layer_travel_acceleration_mm_s2: 0.0,
            travel_short_distance_acceleration_mm_s2: 250.0,
            retract_acceleration_mm_s2: 0.0,
            machine_limits: MachineLimits::default(),
            filament_density_g_cm3: 1.24,
            fan_min_speed: 20,
            fan_max_speed: 100,
            enable_overhang_bridge_fan: true,
            overhang_fan_speed: 100,
            overhang_fan_threshold: OverhangFanThreshold::Bridge,
            ironing_fan_speed: -1,
            close_fan_the_first_x_layers: 1,
            first_x_layer_part_fan_speed: 0,
            full_fan_speed_layer: 0,
            fan_cooling_layer_time_s: 60.0,
            slow_down_layer_time_s: 8.0,
            slow_down_for_layer_cooling: false,
            no_slow_down_for_cooling_on_outwalls: false,
            cooling_slowdown_logic: CoolingSlowdownLogic::Uniform,
            cooling_perimeter_transition_distance_mm: 0.0,
            slow_down_min_speed_mm_s: 10.0,
            reduce_fan_stop_start_freq: false,
            auxiliary_fan: false,
            additional_cooling_fan_speed: 0,
            close_additional_fan_first_x_layers: 1,
            additional_fan_full_speed_layer: 0,
            first_x_layer_fan_speed: 0,
            pre_start_fan_time_s: 0.0,
            support_air_filtration: false,
            activate_air_filtration: false,
            during_print_exhaust_fan_speed: 60,
            complete_print_exhaust_fan_speed: 80,
            filament_max_volumetric_speed_mm3_s: 0.0,
            retraction_length_mm: 0.8,
            retraction_speed_mm_s: 30.0,
            deretraction_speed_mm_s: 0.0,
            retraction_minimum_travel_mm: 2.0,
            reduce_infill_retraction_mode: ReduceInfillRetractionMode::Auto,
            reduce_crossing_wall: false,
            max_travel_detour_distance: 0.0,
            max_travel_detour_is_percent: false,
            avoid_crossing_wall_includes_support: false,
            filament_metal_stickiness: FilamentMetalStickiness::None,
            retract_when_changing_layer: false,
            wipe: false,
            wipe_distance_mm: 2.0,
            retract_before_wipe: 1.0,
            wipe_speed_percent: 80.0,
            role_base_wipe_speed: true,
            retract_restart_extra_mm: 0.0,
            z_hop_mm: 0.4,
            z_hop_type: ZHopType::Spiral,
            retract_lift_above_mm: 0.0,
            retract_lift_below_mm: 0.0,
            travel_speed_z_mm_s: 0.0,
            bed_bbox_valid: false,
            bed_min_x: 0.0,
            bed_min_y: 0.0,
            bed_max_x: 0.0,
            bed_max_y: 0.0,
            xy_jerk_mm_s: 9.0,
            z_jerk_mm_s: 3.0,
            e_jerk_mm_s: 2.5,
            gcode_flavor: GCodeFlavor::Marlin,
            layer_change_gcode: String::new(),
            machine_end_gcode: String::new(),
            filament_end_gcode: String::new(),
            long_retraction_when_cut: false,
            long_retraction_when_ec: false,
            retraction_distance_when_cut: 0.0,
            retraction_distance_when_ec: 0.0,
            machine_start_gcode: String::new(),
            filament_start_gcode: String::new(),
            filament_type: String::from("PLA"),
            filament_vendor: String::from("Generic"),
            chamber_temperature_c: 0,
            temperature_vitrification_c: 45,
            cooling_filter_enabled: false,
            curr_bed_type: String::from("Cool Plate"),
            cool_plate: PlateBedTemps::default(),
            eng_plate: PlateBedTemps::default(),
            hot_plate: PlateBedTemps::default(),
            textured_plate: PlateBedTemps::default(),
            supertack_plate: PlateBedTemps::default(),
            nozzle_temperature_range_high: 240,
            time_lapse_gcode: String::new(),
            timelapse_type: 0,
            farthest_point_timelapse: false,
            spiral_mode: false,
            enable_arc_fitting: false,
            resolution_mm: 0.01,
            enable_wrapping_detection: false,
            wrapping_detection_gcode: String::new(),
            filament_map: vec![1],
            physical_extruder_map: vec![0],
            scan_first_layer: false,
            printer_structure: String::from("undefine"),
            print_sequence: String::from("by layer"),
            skirt_per_object: true,
            standby_temperature_delta_c: -5,
            independent_support_layer_height: true,
            filament_shrink_percent: 100.0,
            printable_height_mm: 250.0,
            printable_area: Vec::new(),
            extruder_printable_areas: Vec::new(),
            bed_exclude_area: Vec::new(),
            extruder_clearance_max_radius_mm: 65.0,
            enable_prime_tower: false,
            wipe_tower_x_mm: 15.0,
            wipe_tower_y_mm: 220.0,
            prime_tower_width_mm: 35.0,
            prime_tower_brim_width_mm: 3.0,
            prime_tower_max_speed_mm_s: 90.0,
            wipe_tower_no_sparse_layers: false,
            enable_tower_interface_features: false,
            prime_tower_lift_height_mm: -1.0,
            prime_tower_lift_speed_mm_s: 90.0,
            prime_tower_enable_framework: false,
            prime_tower_flat_ironing: false,
            flush_into_objects: false,
            flush_into_infill: false,
            flush_into_support: false,
            flush_volumes_mm3: Vec::new(),
            filament_count: 1,
        }
    }
}

impl SliceSettings {
    pub fn infill_spacing_mm(&self) -> f64 {
        self.infill_spacing_for(false)
    }

    pub fn infill_spacing_for(&self, first_layer: bool) -> f64 {
        if self.infill_density <= 0.0 {
            f64::INFINITY
        } else {
            let w = self.line_width_for(FlowRole::SparseInfill, first_layer);
            (w / self.infill_density).max(w)
        }
    }

    /// C++ Fill.cpp `line_spacing = spacing / density` for external top/bottom
    /// (not bridges). `None` skips fill when density is 0.
    pub fn surface_fill_spacing_mm(width_mm: f64, density: f64) -> Option<f64> {
        if density <= 0.0 || !width_mm.is_finite() || width_mm <= 0.0 {
            None
        } else {
            Some(width_mm / density.min(1.0))
        }
    }

    /// C++ `FillParams::multiline` for sparse infill. Concentric stays 1.
    pub fn sparse_fill_multiline(&self) -> u32 {
        if self.infill_pattern.supports_multiline() {
            self.fill_multiline.clamp(1, 5)
        } else {
            1
        }
    }

    /// C++ `PrintRegion::flow` extrusion width. First-layer infill roles use
    /// `initial_layer_infill_line_width` when that value is > 0, then
    /// `initial_layer_line_width` when that value is > 0; role-specific 0
    /// falls back to [`Self::line_width_mm`].
    pub fn line_width_for(&self, role: FlowRole, first_layer: bool) -> f64 {
        if first_layer {
            let infill_role = matches!(
                role,
                FlowRole::SparseInfill | FlowRole::SolidInfill | FlowRole::TopSolidInfill
            );
            if infill_role && self.initial_layer_infill_line_width_mm > 0.0 {
                return self.initial_layer_infill_line_width_mm;
            }
            if self.initial_layer_line_width_mm > 0.0 {
                return self.initial_layer_line_width_mm;
            }
        }
        let specific = match role {
            FlowRole::ExternalPerimeter => self.outer_wall_line_width_mm,
            FlowRole::Perimeter => self.inner_wall_line_width_mm,
            FlowRole::SparseInfill => self.sparse_infill_line_width_mm,
            FlowRole::SolidInfill => self.internal_solid_infill_line_width_mm,
            FlowRole::TopSolidInfill => self.top_surface_line_width_mm,
            FlowRole::SupportMaterial => self.support_line_width_mm,
        };
        if specific > 0.0 {
            specific
        } else {
            self.line_width_mm
        }
    }

    /// C++ `GCode::_extrude`: `print_flow_ratio` first, then top solid uses
    /// `top_solid_infill_flow_ratio`; other first-layer paths use
    /// `initial_layer_flow_ratio`.
    pub fn gcode_path_flow_factor(&self, role: FlowRole, first_layer: bool) -> f64 {
        let print = self.print_flow_ratio.max(0.0);
        let role_or_first = if role == FlowRole::TopSolidInfill {
            self.top_solid_infill_flow_ratio.max(0.0)
        } else if first_layer {
            self.initial_layer_flow_ratio.max(0.0)
        } else {
            1.0
        };
        print * role_or_first
    }

    /// C++ Classic `is_outer_wall_first` for the two FEATURE buckets.
    ///
    /// InnerOuterInner weaves remaining inners, outer, then first inner.
    /// The layer-0 `btOuterOnly` brim reverse is skipped: BBL brim is `auto_brim`.
    pub fn outer_walls_first(&self) -> bool {
        matches!(self.wall_sequence, WallSequence::OuterInner)
    }

    /// C++ classic `WallSequence::InnerOuterInner`.
    pub fn weaves_inner_outer_inner(&self) -> bool {
        matches!(self.wall_sequence, WallSequence::InnerOuterInner)
    }

    /// C++ `scale_(nozzle_diameter) * (seam_gap / 100)` in millimetres.
    pub fn seam_gap_mm(&self) -> f64 {
        self.nozzle_diameter_mm.max(0.0) * self.seam_gap.max(0.0)
    }

    /// C++ `override_filament_scarf_seam_setting` vs `filament_scarf_seam_type`.
    pub fn effective_seam_slope_type(&self) -> SeamScarfType {
        if self.override_filament_scarf_seam_setting {
            self.seam_slope_type
        } else {
            self.filament_scarf_seam_type
        }
    }

    /// C++ scarf start as a 0–1 fraction of the current layer height.
    pub fn scarf_start_ratio(&self, layer_height_mm: f64) -> f64 {
        let h = layer_height_mm.max(1e-9);
        let (value, is_percent) = if self.override_filament_scarf_seam_setting {
            (
                self.seam_slope_start_height,
                self.seam_slope_start_height_is_percent,
            )
        } else {
            (
                self.filament_scarf_height,
                self.filament_scarf_height_is_percent,
            )
        };
        let mm = if is_percent { h * value / 100.0 } else { value };
        (mm / h).clamp(0.0, 1.0)
    }

    /// C++ `seam_slope_min_length` vs `filament_scarf_length`.
    pub fn effective_scarf_length_mm(&self) -> f64 {
        if self.override_filament_scarf_seam_setting {
            self.seam_slope_min_length_mm
        } else {
            self.filament_scarf_length_mm
        }
    }

    /// C++ `seam_slope_gap.get_abs_value(nozzle)` vs filament scarf gap.
    pub fn effective_scarf_gap_mm(&self) -> f64 {
        let nozzle = self.nozzle_diameter_mm.max(0.0);
        let (value, is_percent) = if self.override_filament_scarf_seam_setting {
            (self.seam_slope_gap, self.seam_slope_gap_is_percent)
        } else {
            (self.filament_scarf_gap, self.filament_scarf_gap_is_percent)
        };
        if is_percent {
            nozzle * value / 100.0
        } else {
            value
        }
        .max(0.0)
    }

    /// C++ turning angle at the seam vs `scarf_angle_threshold`.
    pub fn scarf_angle_allows(&self, seam_interior_deg: f64) -> bool {
        if !self.seam_slope_conditional {
            return true;
        }
        seam_interior_deg + 1e-6 >= f64::from(self.scarf_angle_threshold_deg)
    }

    /// C++ `enable_seam_slope` role/type gate without hole detection.
    pub fn scarf_applies_to_wall(&self, external_perimeter: bool, is_hole: bool) -> bool {
        let ty = self.effective_seam_slope_type();
        let type_ok = match ty {
            SeamScarfType::None => false,
            SeamScarfType::External => !is_hole,
            SeamScarfType::All => true,
        };
        type_ok && (external_perimeter || self.seam_slope_inner_walls)
    }

    /// C++ filament overhang bands when `override_process_overhang_speed`.
    pub fn uses_filament_overhang_speed(&self) -> bool {
        self.override_process_overhang_speed && self.filament_enable_overhang_speed
    }

    pub fn overhang_band_speed_mm_s(&self, degree: u8) -> f64 {
        if self.uses_filament_overhang_speed() {
            match degree {
                1 => self.filament_overhang_1_4_speed_mm_s,
                2 => self.filament_overhang_2_4_speed_mm_s,
                3 => self.filament_overhang_3_4_speed_mm_s,
                4 => self.filament_overhang_4_4_speed_mm_s,
                _ => self.filament_overhang_speed_mm_s,
            }
        } else {
            match degree {
                1 => self.overhang_1_4_speed_mm_s,
                2 => self.overhang_2_4_speed_mm_s,
                3 => self.overhang_3_4_speed_mm_s,
                4 => self.overhang_4_4_speed_mm_s,
                _ => self.overhang_speed_mm_s,
            }
        }
    }

    pub fn effective_bridge_speed_mm_s(&self) -> f64 {
        if self.uses_filament_overhang_speed() && self.filament_bridge_speed_mm_s > 0.0 {
            self.filament_bridge_speed_mm_s
        } else {
            self.bridge_speed_mm_s
        }
    }

    /// C++ height slowdown lerp between start/end height. 1.0 when disabled.
    pub fn height_slowdown_t(&self, print_z_mm: f64) -> f64 {
        if !self.enable_height_slowdown {
            return 0.0;
        }
        let span = self.slowdown_end_height_mm - self.slowdown_start_height_mm;
        if span.abs() < 1e-9 {
            return 0.0;
        }
        ((print_z_mm - self.slowdown_start_height_mm) / span).clamp(0.0, 1.0)
    }

    pub fn height_slowdown_speed_mm_s(&self, print_z_mm: f64, base_mm_s: f64) -> f64 {
        if !self.enable_height_slowdown {
            return base_mm_s;
        }
        let t = self.height_slowdown_t(print_z_mm);
        let start = if self.slowdown_start_speed_mm_s > 0.0 {
            self.slowdown_start_speed_mm_s
        } else {
            base_mm_s
        };
        let end = if self.slowdown_end_speed_mm_s > 0.0 {
            self.slowdown_end_speed_mm_s
        } else {
            base_mm_s
        };
        start + (end - start) * t
    }

    pub fn height_slowdown_acc_mm_s2(&self, print_z_mm: f64, base_acc: f64) -> f64 {
        if !self.enable_height_slowdown {
            return base_acc;
        }
        let t = self.height_slowdown_t(print_z_mm);
        let start = if self.slowdown_start_acc_mm_s2 > 0.0 {
            self.slowdown_start_acc_mm_s2
        } else {
            base_acc
        };
        let end = if self.slowdown_end_acc_mm_s2 > 0.0 {
            self.slowdown_end_acc_mm_s2
        } else {
            base_acc
        };
        start + (end - start) * t
    }

    /// C++ `filament_shrink` as an XY scale (100% → 1.0).
    pub fn filament_xy_shrink_scale(&self) -> f64 {
        if self.filament_shrink_percent <= 1e-9 {
            1.0
        } else {
            100.0 / self.filament_shrink_percent
        }
    }

    /// C++ `PerimeterGenerator` wall count: `wall_loops`, plus one on odd
    /// layers when [`Self::alternate_extra_wall`] is on and not spiral vase.
    pub fn wall_loops_for_layer(&self, layer_idx: usize) -> u32 {
        let n = self.wall_loops.max(1);
        if self.alternate_extra_wall && layer_idx % 2 == 1 && !self.spiral_mode {
            n.saturating_add(1)
        } else {
            n
        }
    }

    /// C++ `Print::has_infinite_skirt`.
    pub fn has_infinite_skirt(&self) -> bool {
        (self.draft_shield == DraftShield::Enabled && self.skirt_loops > 0)
            || (self.ooze_prevention && self.filament_count > 1)
    }

    /// C++ `Print::has_skirt`.
    pub fn has_skirt(&self) -> bool {
        (self.skirt_height > 0 && self.skirt_loops > 0)
            || self.draft_shield != DraftShield::Disabled
    }

    /// Outer brim loops from `brim_width` (C++ `brim_type != btNoBrim` / inner-only).
    pub fn has_outer_brim(&self) -> bool {
        self.brim_width_mm > 0.0 && self.brim_type.has_outer()
    }

    /// C++ `gap_xy` vs `gap_xy_first_layer` (`support_object_first_layer_gap`).
    pub fn support_xy_gap_mm(&self, object_layer_idx: usize) -> f64 {
        if object_layer_idx == 0 {
            self.support_object_first_layer_gap_mm
        } else {
            self.support_xy_distance_mm
        }
    }

    /// C++ `gap_support_object` as a whole-layer count. Zero is soluble.
    pub fn support_top_gap_layers(&self) -> usize {
        Self::z_gap_layers(self.support_top_z_distance_mm, self.layer_height_mm)
    }

    /// C++ `gap_object_support`. `<= 0` copies [`Self::support_top_z_distance_mm`].
    pub fn resolved_support_bottom_z_distance_mm(&self) -> f64 {
        if self.support_bottom_z_distance_mm <= 0.0 {
            self.support_top_z_distance_mm
        } else {
            self.support_bottom_z_distance_mm
        }
    }

    /// C++ `gap_object_support` as a whole-layer count. Zero is soluble.
    pub fn support_bottom_gap_layers(&self) -> usize {
        Self::z_gap_layers(
            self.resolved_support_bottom_z_distance_mm(),
            self.layer_height_mm,
        )
    }

    /// C++ `std::round(gap / layer_height + EPSILON)` when support shares object Z.
    fn z_gap_layers(gap_mm: f64, layer_height_mm: f64) -> usize {
        if gap_mm <= 1e-9 {
            0
        } else {
            (gap_mm / layer_height_mm.max(1e-6) + 1e-4).round() as usize
        }
    }

    /// C++ `TreeSupport::calc_branch_radius` (`mm_to_top` form). Interface
    /// layers keep at least the contact radius.
    pub fn tree_branch_radius_mm(&self, mm_to_top: f64) -> f64 {
        let base = (self.tree_branch_diameter_mm.max(self.line_width_mm) * 0.5).max(0.4);
        let tan_d = self
            .tree_branch_diameter_angle_deg
            .max(0.0)
            .to_radians()
            .tan();
        let mut radius = if mm_to_top > base {
            base + (mm_to_top - base) * tan_d
        } else {
            mm_to_top
        };
        radius = radius.clamp(0.4, 10.0);
        if self.support_interface_layers > 0 {
            radius.max(base)
        } else {
            radius
        }
    }

    /// C++ `LayerRegion` custom bridge angle when `bridge_angle > 0`.
    pub fn bridge_fill_angle_deg(&self, bridged: bool) -> f64 {
        if bridged && self.bridge_angle_deg > 0.0 {
            self.bridge_angle_deg
        } else {
            self.infill_direction_deg
        }
    }

    /// C++ `Fill.cpp` ironing: `(int(ironing_direction + infill_direction) % 180)`.
    pub fn ironing_angle_deg(&self) -> f64 {
        (self.ironing_direction_deg + self.infill_direction_deg).rem_euclid(180.0)
    }

    /// C++ other-layer `skirt_done.size() < skirt_height || has_infinite_skirt`,
    /// plus the first-layer skirt when `has_skirt`.
    pub fn skirt_layer_count(&self, layer_count: usize) -> usize {
        if self.skirt_loops == 0 {
            return 0;
        }
        if self.has_infinite_skirt() {
            return layer_count;
        }
        if !self.has_skirt() {
            return 0;
        }
        let n = self.skirt_height as usize;
        if n == 0 {
            1.min(layer_count)
        } else {
            n.min(layer_count)
        }
    }

    /// C++ `GCode::needs_retract` reduce-infill branch (`rirEnabled` / Auto+Low/None).
    pub fn should_reduce_infill_retraction(&self) -> bool {
        match self.reduce_infill_retraction_mode {
            ReduceInfillRetractionMode::Disabled => false,
            ReduceInfillRetractionMode::Enabled => true,
            ReduceInfillRetractionMode::Auto => {
                self.filament_metal_stickiness.is_low_for_infill_retract()
            }
        }
    }

    /// C++ `max_travel_detour_distance`: 0 disables the cap; percent is of the direct hop.
    pub fn max_travel_detour_limit_mm(&self, direct_mm: f64) -> f64 {
        if self.max_travel_detour_distance <= 0.0 {
            f64::INFINITY
        } else if self.max_travel_detour_is_percent {
            direct_mm * self.max_travel_detour_distance / 100.0
        } else {
            self.max_travel_detour_distance
        }
    }

    /// C++ `LayerRegion::simplify_path` / `Layer::simplify_support_path` arc-fit epsilon.
    /// Sparse infill uses 0.04 mm; support uses 0.0375 mm; everything else uses `resolution`.
    pub fn arc_fit_tolerance_mm(&self, role: FlowRole) -> f64 {
        match role {
            FlowRole::SparseInfill => SPARSE_INFILL_RESOLUTION_MM,
            FlowRole::SupportMaterial => SUPPORT_RESOLUTION_MM,
            _ => self.resolution_mm.max(0.001),
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
        let w = self.line_width_for(FlowRole::SupportMaterial, false);
        // C++ `support_spacing = support_base_pattern_spacing + flow.spacing()`.
        // Keep density spacing at the BBL default 2.5 so existing hatch does not move.
        if (self.support_base_pattern_spacing_mm - 2.5).abs() > 1e-9 {
            (self.support_base_pattern_spacing_mm.max(0.0) + w).max(w)
        } else if self.support_density <= 0.0 {
            f64::INFINITY
        } else {
            (w / self.support_density).max(w)
        }
    }

    /// C++ `interface_spacing = support_interface_spacing + flow.spacing()`.
    /// Keep `width × 1.1` at the BBL default 0.5 so existing hatch does not move.
    pub fn support_interface_hatch_spacing_mm(&self) -> f64 {
        Self::support_hatch_spacing_mm(
            self.support_interface_spacing_mm,
            self.line_width_for(FlowRole::SupportMaterial, false),
        )
    }

    /// C++ `support_bottom_interface_spacing + flow.spacing()`, same 0.5 default
    /// as top so BBL-omitted floors keep `width × 1.1`.
    pub fn support_bottom_interface_hatch_spacing_mm(&self) -> f64 {
        Self::support_hatch_spacing_mm(
            self.support_bottom_interface_spacing_mm,
            self.line_width_for(FlowRole::SupportMaterial, false),
        )
    }

    fn support_hatch_spacing_mm(spacing_mm: f64, width_mm: f64) -> f64 {
        if spacing_mm.abs() < 1e-9 {
            width_mm
        } else if (spacing_mm - 0.5).abs() > 1e-9 {
            (spacing_mm.max(0.0) + width_mm).max(width_mm)
        } else {
            (width_mm * 1.1).max(width_mm)
        }
    }

    /// C++ `num_bottom_interface_layers`: `-1` copies top layer count.
    pub fn resolved_support_interface_bottom_layers(&self) -> u32 {
        if self.support_interface_bottom_layers < 0 {
            self.support_interface_layers
        } else {
            self.support_interface_bottom_layers as u32
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

    /// C++ `clamp_exturder_to_default_protect0`: 0 / negative becomes filament 1.
    pub fn clamp_print_filaments(&mut self) {
        if self.wall_filament < 1 {
            self.wall_filament = 1;
        }
        if self.sparse_infill_filament < 1 {
            self.sparse_infill_filament = 1;
        }
        if self.solid_infill_filament < 1 {
            self.solid_infill_filament = 1;
        }
    }

    /// C++ `vertical_shell_speed.get_abs_value(internal_solid_infill_speed)`.
    pub fn vertical_shell_speed_mm_s(&self) -> f64 {
        if self.vertical_shell_speed_is_percent {
            self.vertical_shell_speed * self.solid_infill_speed_mm_s / 100.0
        } else {
            self.vertical_shell_speed
        }
    }

    /// C++ `Extruder::deretract_speed`: zero deretraction uses retraction speed.
    pub fn deretract_speed_mm_s(&self) -> f64 {
        if self.deretraction_speed_mm_s > 0.0 {
            self.deretraction_speed_mm_s
        } else {
            self.retraction_speed_mm_s
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

    fn sparse_infill_acceleration_mm_s2(&self) -> f64 {
        let v = self.sparse_infill_acceleration;
        if v <= 0.0 {
            0.0
        } else if self.sparse_infill_acceleration_is_percent {
            v / 100.0 * self.default_acceleration_mm_s2
        } else {
            v
        }
    }

    /// C++ `GCode::extrude` acceleration for a path role.
    pub fn print_acceleration_mm_s2(&self, first_layer: bool, kind: PrintAccel) -> f64 {
        if first_layer && self.initial_layer_acceleration_mm_s2 > 0.0 {
            return self.initial_layer_acceleration_mm_s2;
        }
        let role = match kind {
            PrintAccel::Default => 0.0,
            PrintAccel::OuterWall => self.outer_wall_acceleration_mm_s2,
            PrintAccel::InnerWall => self.inner_wall_acceleration_mm_s2,
            PrintAccel::TopSurface => self.top_surface_acceleration_mm_s2,
            PrintAccel::SparseInfill => self.sparse_infill_acceleration_mm_s2(),
        };
        if role > 0.0 {
            role
        } else {
            self.default_acceleration_mm_s2
        }
    }

    /// C++ travel acceleration, including the first-layer override.
    pub fn travel_acceleration_for_layer(&self, first_layer: bool) -> f64 {
        if first_layer && self.initial_layer_travel_acceleration_mm_s2 > 0.0 {
            self.initial_layer_travel_acceleration_mm_s2
        } else if self.travel_acceleration_mm_s2 > 0.0 {
            self.travel_acceleration_mm_s2
        } else {
            self.default_acceleration_mm_s2
        }
    }

    /// C++ short-travel accel for hops to outer walls shorter than `retraction_minimum_travel`.
    pub fn travel_acceleration_for_move(
        &self,
        first_layer: bool,
        to_outer_wall: bool,
        dist_mm: f64,
    ) -> f64 {
        if !first_layer
            && to_outer_wall
            && self.travel_short_distance_acceleration_mm_s2 > 0.0
            && dist_mm + 1e-9 < self.retraction_minimum_travel_mm
        {
            return self.travel_short_distance_acceleration_mm_s2;
        }
        self.travel_acceleration_for_layer(first_layer)
    }

    /// C++ `get_retract_acceleration`: machine limit, else 1500.
    pub fn retract_acceleration_or_default(&self) -> f64 {
        if self.retract_acceleration_mm_s2 > 0.0 {
            self.retract_acceleration_mm_s2
        } else {
            1500.0
        }
    }

    /// C++ `GCode::print_machine_envelope` for Marlin flavors.
    pub fn print_machine_envelope(&self) -> Option<String> {
        if self.gcode_flavor != GCodeFlavor::Marlin {
            return None;
        }
        let lim = &self.machine_limits;
        let p = round_machine_limit(lim.acceleration_extruding_mm_s2);
        let r = round_machine_limit(self.retract_acceleration_or_default());
        Some(format!(
            "M201 X{} Y{} Z{} E{}\nM203 X{} Y{} Z{} E{}\nM204 P{} R{} T{}\nM205 X{:.2} Y{:.2} Z{:.2} E{:.2}\n",
            round_machine_limit(lim.acceleration_x_mm_s2),
            round_machine_limit(lim.acceleration_y_mm_s2),
            round_machine_limit(lim.acceleration_z_mm_s2),
            round_machine_limit(lim.acceleration_e_mm_s2),
            round_machine_limit(lim.speed_x_mm_s),
            round_machine_limit(lim.speed_y_mm_s),
            round_machine_limit(lim.speed_z_mm_s),
            round_machine_limit(lim.speed_e_mm_s),
            p,
            r,
            p,
            self.xy_jerk_mm_s,
            self.xy_jerk_mm_s,
            self.z_jerk_mm_s,
            self.e_jerk_mm_s,
        ))
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

    /// C++ `GCode.cpp` overhang/bridge fan marker predicate.
    pub fn overhang_fan_applies(&self, degree: u8, is_bridge: bool, is_external: bool) -> bool {
        if !self.enable_overhang_bridge_fan {
            return false;
        }
        if is_bridge {
            return true;
        }
        let none = self.overhang_fan_threshold == OverhangFanThreshold::None;
        if none && is_external {
            return true;
        }
        degree > self.overhang_fan_threshold.compare_degree()
    }

    /// C++ `get_bed_temp_key` / `get_bed_temp_1st_layer_key` for `curr_bed_type`.
    pub fn plate_bed_temps(&self) -> PlateBedTemps {
        match self.curr_bed_type.as_str() {
            "Engineering Plate" => self.eng_plate,
            "High Temp Plate" => self.hot_plate,
            "Textured PEI Plate" => self.textured_plate,
            "Supertack Plate" => self.supertack_plate,
            _ => self.cool_plate,
        }
    }

    /// Copy the active plate's temps into `bed_temperature_c` / initial.
    /// Leaves the generic 60 °C defaults when that plate is unset (0).
    pub fn resolve_bed_temps_from_plate(&mut self) {
        let plate = self.plate_bed_temps();
        if plate.later_c > 0 {
            self.bed_temperature_c = plate.later_c;
        }
        if plate.initial_c > 0 {
            self.bed_temperature_initial_layer_c = plate.initial_c;
        } else if plate.later_c > 0 {
            self.bed_temperature_initial_layer_c = plate.later_c;
        }
    }

    /// C++ `PrinterStructure::psI3`.
    pub fn is_i3(&self) -> bool {
        self.printer_structure.eq_ignore_ascii_case("i3")
    }

    /// C++ `m_farthest_point_timelapse.enabled` (toggle + traditional + not I3).
    pub fn farthest_point_timelapse_enabled(&self) -> bool {
        self.farthest_point_timelapse && self.timelapse_type == 0 && !self.is_i3()
    }

    /// C++ `print_sequence == PrintSequence::ByObject`.
    pub fn print_sequence_by_object(&self) -> bool {
        self.print_sequence.to_ascii_lowercase().contains("object")
    }

    /// C++ `Print::has_wipe_tower`.
    pub fn has_wipe_tower(&self) -> bool {
        if !self.enable_prime_tower {
            return false;
        }
        if self.timelapse_type == 1 {
            return true;
        }
        !self.spiral_mode && self.filament_count > 1
    }

    /// Flush from 1-based filament `from` into `to` (mm³). Diagonal is 0.
    pub fn flush_volume_mm3(&self, from_1based: i32, to_1based: i32) -> f64 {
        if from_1based == to_1based {
            return 0.0;
        }
        let n = self.filament_count.max(1);
        let i = (from_1based.max(1) as usize - 1).min(n - 1);
        let j = (to_1based.max(1) as usize - 1).min(n - 1);
        let idx = i * n + j;
        self.flush_volumes_mm3
            .get(idx)
            .copied()
            .filter(|v| *v > 0.0)
            .unwrap_or(800.0)
    }

    pub fn max_flush_volume_mm3(&self) -> f64 {
        if self.filament_count <= 1 {
            return 0.0;
        }
        let n = self.filament_count as i32;
        (1..=n)
            .flat_map(|a| (1..=n).map(move |b| self.flush_volume_mm3(a, b)))
            .fold(0.0, f64::max)
    }

    /// C++ `wipe_tower_sparse_layers_skipped`.
    pub fn wipe_tower_skips_sparse_layers(&self) -> bool {
        self.wipe_tower_no_sparse_layers
            && self.timelapse_type != 1
            && !self.enable_wrapping_detection
    }

    /// C++ `wipe_tower_data().bbx` stand-in: width square plus brim, local to the tower.
    pub fn wipe_tower_bbx_mm(&self) -> ((f64, f64), (f64, f64)) {
        let brim = self.prime_tower_brim_width_mm.max(0.0);
        let width = self.prime_tower_width_mm.max(0.0);
        ((-brim, -brim), (width + brim, width + brim))
    }

    /// Center of [`Self::wipe_tower_bbx_mm`] in world millimetres.
    pub fn wipe_tower_center_mm(&self) -> (f64, f64) {
        let ((min_x, min_y), (max_x, max_y)) = self.wipe_tower_bbx_mm();
        (
            self.wipe_tower_x_mm + (min_x + max_x) * 0.5,
            self.wipe_tower_y_mm + (min_y + max_y) * 0.5,
        )
    }

    /// C++ `during_print_exhaust_fan_speed_num` (percent → 0–255 PWM).
    pub fn during_print_exhaust_fan_speed_num(&self) -> i32 {
        (f64::from(self.during_print_exhaust_fan_speed) / 100.0 * 255.0) as i32
    }

    /// C++ `get_outer_wall_volumetric_speed` (stadium flow, no flow ratio).
    pub fn outer_wall_volumetric_speed(&self) -> f64 {
        let width = {
            let w = self.line_width_for(FlowRole::ExternalPerimeter, false);
            if w > 0.0 {
                w
            } else {
                self.filament_diameter_mm
            }
        };
        let h = self.layer_height_mm;
        let mm3_per_mm = h * (width - h * (1.0 - 0.25 * std::f64::consts::PI));
        let mut vol = self.print_speed_mm_s * mm3_per_mm.max(0.0);
        if self.filament_max_volumetric_speed_mm3_s > 0.0 {
            vol = vol.min(self.filament_max_volumetric_speed_mm3_s);
        }
        vol
    }

    /// C++ `physical_extruder_map.get_at(filament_id)`.
    pub fn physical_extruder_id(&self, filament_id: usize) -> i32 {
        match self.physical_extruder_map.as_slice() {
            [] => 0,
            map => map[filament_id.min(map.len() - 1)],
        }
    }

    /// C++ `Extruder::extruder_id`: `filament_map[filament] - 1`.
    pub fn filament_extruder_index(&self, filament_id: usize) -> usize {
        let raw = match self.filament_map.as_slice() {
            [] => 1,
            map => map[filament_id.min(map.len() - 1)],
        };
        (raw.max(1) as usize).saturating_sub(1)
    }

    /// C++ `T` number for a 1-based filament id (`filament_map`, else the id).
    pub fn tool_command_for_filament(&self, filament_1based: i32) -> i32 {
        if filament_1based < 1 {
            return self.filament_map.first().copied().unwrap_or(1);
        }
        let idx = (filament_1based as usize) - 1;
        self.filament_map
            .get(idx)
            .copied()
            .unwrap_or(filament_1based)
    }

    /// C++ `nozzle_diameter.size()`.
    pub fn nozzle_count(&self) -> usize {
        self.nozzle_diameters_mm.len().max(1)
    }

    /// Square bed edge from `printable_area`, else 256 mm (X1/P1).
    pub fn bed_size_mm(&self) -> f32 {
        if self.printable_area.len() < 2 {
            return 256.0;
        }
        let mut min_x = f64::INFINITY;
        let mut max_x = f64::NEG_INFINITY;
        let mut min_y = f64::INFINITY;
        let mut max_y = f64::NEG_INFINITY;
        for &(x, y) in &self.printable_area {
            min_x = min_x.min(x);
            max_x = max_x.max(x);
            min_y = min_y.min(y);
            max_y = max_y.max(y);
        }
        let edge = (max_x - min_x).max(max_y - min_y);
        if edge.is_finite() && edge > 1.0 {
            edge as f32
        } else {
            256.0
        }
    }

    /// C++ `first_filaments` after `match_physical_extruder_for_each_filament`.
    pub fn first_filaments(&self) -> Vec<i32> {
        remap_filaments_to_physical(
            &self.first_filaments_by_logical_slot(),
            &self.physical_extruder_map,
        )
    }

    /// C++ `first_non_support_filaments` after the same physical remap.
    pub fn first_non_support_filaments(&self) -> Vec<i32> {
        self.first_filaments()
    }

    /// C++ `first_non_support_hotend` (group id, or -1 when the slot is unused).
    pub fn first_non_support_hotends(&self) -> Vec<i32> {
        self.first_non_support_filaments()
            .into_iter()
            .map(|id| if id < 0 { -1 } else { 0 })
            .collect()
    }

    fn first_filaments_by_logical_slot(&self) -> Vec<i32> {
        let n = self.nozzle_count();
        let mut first = vec![-1; n];
        let Some(&mapped) = self.filament_map.first() else {
            return first;
        };
        let extruder_id = mapped - 1;
        if extruder_id >= 0 {
            let e = extruder_id as usize;
            if e < n {
                first[e] = 0;
            }
        }
        first
    }

    /// Bambu `0.20mm Standard @BBL H2C` plus H2C 0.4 nozzle and Generic PLA @ H2C 0.4.
    pub fn bbl_0_20() -> Self {
        Self {
            infill_density: 0.15,
            infill_pattern: InfillPattern::Grid,
            skirt_loops: 0,
            brim_width_mm: 5.0,
            brim_object_gap_mm: 0.1,
            initial_layer_line_width_mm: 0.5,
            initial_layer_infill_line_width_mm: 0.5,
            inner_wall_line_width_mm: 0.45,
            outer_wall_line_width_mm: 0.42,
            sparse_infill_line_width_mm: 0.45,
            internal_solid_infill_line_width_mm: 0.42,
            top_surface_line_width_mm: 0.42,
            support_line_width_mm: 0.42,
            top_shell_layers: 5,
            top_shell_thickness_mm: 1.0,
            top_surface_pattern: SurfacePattern::MonotonicLine,
            bottom_surface_pattern: SurfacePattern::Monotonic,
            elephant_foot_mm: 0.15,
            top_one_wall: TopOneWallType::AllTop,
            support_type: SupportType::Tree,
            enable_arc_fitting: true,
            resolution_mm: 0.012,
            travel_speed_mm_s: 1000.0,
            print_speed_mm_s: 200.0,
            inner_wall_speed_mm_s: 300.0,
            first_layer_speed_mm_s: 50.0,
            first_layer_infill_speed_mm_s: 105.0,
            infill_speed_mm_s: 350.0,
            gap_infill_speed_mm_s: 250.0,
            solid_infill_speed_mm_s: 250.0,
            support_speed_mm_s: 150.0,
            support_interface_speed_mm_s: 80.0,
            enable_overhang_speed: true,
            overhang_1_4_speed_mm_s: 0.0,
            overhang_2_4_speed_mm_s: 50.0,
            overhang_3_4_speed_mm_s: 30.0,
            overhang_4_4_speed_mm_s: 10.0,
            overhang_speed_mm_s: 10.0,
            bridge_speed_mm_s: 50.0,
            top_surface_speed_mm_s: 200.0,
            default_acceleration_mm_s2: 8000.0,
            outer_wall_acceleration_mm_s2: 5000.0,
            initial_layer_acceleration_mm_s2: 500.0,
            top_surface_acceleration_mm_s2: 2000.0,
            sparse_infill_acceleration: 100.0,
            sparse_infill_acceleration_is_percent: true,
            travel_acceleration_mm_s2: 10000.0,
            initial_layer_travel_acceleration_mm_s2: 6000.0,
            travel_short_distance_acceleration_mm_s2: 250.0,
            retract_acceleration_mm_s2: 5000.0,
            ironing_flow: 0.15,
            support_ironing_speed_mm_s: 30.0,
            fan_min_speed: 100,
            fan_max_speed: 100,
            close_fan_the_first_x_layers: 1,
            flow_ratio: 0.99,
            overhang_fan_threshold: OverhangFanThreshold::ThreeFour,
            fan_cooling_layer_time_s: 100.0,
            slow_down_layer_time_s: 8.0,
            slow_down_for_layer_cooling: true,
            slow_down_min_speed_mm_s: 20.0,
            reduce_fan_stop_start_freq: true,
            auxiliary_fan: true,
            additional_cooling_fan_speed: 75,
            close_additional_fan_first_x_layers: 1,
            first_x_layer_fan_speed: 0,
            pre_start_fan_time_s: 2.0,
            filament_max_volumetric_speed_mm3_s: 12.0,
            retraction_length_mm: 0.4,
            retraction_speed_mm_s: 30.0,
            deretraction_speed_mm_s: 30.0,
            retraction_minimum_travel_mm: 1.0,
            retract_when_changing_layer: true,
            wipe: true,
            wipe_distance_mm: 1.0,
            retract_before_wipe: 0.0,
            z_hop_mm: 0.4,
            z_hop_type: ZHopType::Spiral,
            retract_lift_below_mm: 319.0,
            bed_bbox_valid: true,
            bed_min_x: 0.0,
            bed_min_y: 0.0,
            bed_max_x: 325.0,
            bed_max_y: 320.0,
            layer_change_gcode: String::from(
                "; layer num/total_layer_count: {layer_num+1}/[total_layer_count]\n; update layer progress\nM73 L{layer_num+1}\nM991 S0 P{layer_num} ;notify layer change",
            ),
            ..Self::default()
        }
    }

    /// Values C++ `PlaceholderParser` sets for custom machine/filament G-code.
    pub fn placeholder_custom_gcode_context(
        &self,
        layer_num: usize,
        total_layers: usize,
        max_layer_z: f64,
        first_layer_min: (f64, f64),
        first_layer_size: (f64, f64),
    ) -> PlaceholderContext {
        let mut ctx = PlaceholderContext::new();
        ctx.set("layer_num", layer_num);
        ctx.set("total_layer_count", total_layers);
        ctx.set("layer_z", max_layer_z);
        ctx.set("max_layer_z", max_layer_z);
        ctx.set("max_print_z", max_layer_z.ceil() as i64);
        ctx.set("current_filament_id", 0);
        ctx.set("current_hotend", 0);
        ctx.set("filament_extruder_id", 0);
        ctx.set("current_extruder_id", 0);
        ctx.set("current_extruder", 0);
        ctx.set("current_nozzle_id", 0);
        ctx.set("initial_nozzle_id", 0);
        ctx.set("initial_no_support_filament_id", 0);
        ctx.set("initial_no_support_hotend", 0);
        ctx.set("initial_filament_id", 0);
        if self.filament_map.is_empty() {
            ctx.set_list("filament_map", [1]);
        } else {
            ctx.set_list("filament_map", self.filament_map.iter().copied());
        }
        ctx.set(
            "long_retraction_when_cut",
            i32::from(self.long_retraction_when_cut),
        );
        ctx.set(
            "long_retraction_when_ec",
            i32::from(self.long_retraction_when_ec),
        );
        ctx.set(
            "retraction_distance_when_cut",
            self.retraction_distance_when_cut,
        );
        ctx.set(
            "retraction_distance_when_ec",
            self.retraction_distance_when_ec,
        );
        ctx.set(
            "retraction_distances_when_cut",
            self.retraction_distance_when_cut,
        );
        ctx.set(
            "flush_volumetric_speeds",
            self.filament_max_volumetric_speed_mm3_s,
        );
        ctx.set(
            "filament_max_volumetric_speed",
            self.filament_max_volumetric_speed_mm3_s,
        );
        let flush_temp = if self.nozzle_temperature_range_high > 0 {
            self.nozzle_temperature_range_high
        } else {
            self.temperature_c
        };
        ctx.set("flush_temperatures", flush_temp);
        ctx.set("nozzle_temperature", self.temperature_c);
        ctx.set(
            "nozzle_temperature_initial_layer",
            self.temperature_initial_layer_c,
        );
        ctx.set("first_layer_temperature", self.temperature_initial_layer_c);
        ctx.set("filament_type", self.filament_type.clone());
        ctx.set(
            "is_all_bbl_filament",
            i32::from(self.filament_vendor == "Bambu Lab"),
        );
        ctx.set("overall_chamber_temperature", self.chamber_temperature_c);
        ctx.set("chamber_temperature", self.chamber_temperature_c);
        ctx.set(
            "min_vitrification_temperature",
            self.temperature_vitrification_c,
        );
        ctx.set(
            "cooling_filter_enabled",
            i32::from(self.cooling_filter_enabled),
        );
        ctx.set("curr_bed_type", self.curr_bed_type.clone());
        ctx.set(
            "bed_temperature_initial_layer_single",
            self.bed_temperature_initial_layer_c,
        );
        ctx.set(
            "bed_temperature_initial_layer",
            self.bed_temperature_initial_layer_c,
        );
        ctx.set(
            "first_layer_bed_temperature",
            self.bed_temperature_initial_layer_c,
        );
        ctx.set("bed_temperature", self.bed_temperature_c);
        let (wt_x, wt_y) = self.wipe_tower_center_mm();
        ctx.set(
            "wipe_tower_center_pos_valid",
            i32::from(self.has_wipe_tower()),
        );
        ctx.set("wipe_tower_center_pos_x", wt_x);
        ctx.set("wipe_tower_center_pos_y", wt_y);
        ctx.set(
            "has_tpu_in_first_layer",
            i32::from(self.filament_type.eq_ignore_ascii_case("TPU")),
        );
        let first_filaments = self.first_filaments();
        let first_non_support = self.first_non_support_filaments();
        let first_hotends = self.first_non_support_hotends();
        ctx.set_list("first_tools", first_filaments.iter().copied());
        ctx.set_list("first_filaments", first_filaments);
        ctx.set_list("first_non_support_tools", first_non_support.iter().copied());
        ctx.set_list("first_non_support_filaments", first_non_support);
        ctx.set_list("first_non_support_hotend", first_hotends);
        if self.nozzle_diameters_mm.is_empty() {
            ctx.set("nozzle_diameter_at_nozzle_id", self.nozzle_diameter_mm);
        } else {
            ctx.set_list(
                "nozzle_diameter_at_nozzle_id",
                self.nozzle_diameters_mm.iter().copied(),
            );
        }
        ctx.set(
            "activate_air_filtration",
            i32::from(self.activate_air_filtration),
        );
        ctx.set(
            "support_air_filtration",
            i32::from(self.support_air_filtration),
        );
        ctx.set(
            "during_print_exhaust_fan_speed_num",
            self.during_print_exhaust_fan_speed_num(),
        );
        ctx.set(
            "outer_wall_volumetric_speed",
            self.outer_wall_volumetric_speed(),
        );
        ctx.set("printer_structure", self.printer_structure.clone());
        ctx.set("print_sequence", self.print_sequence.clone());
        ctx.set("max_print_height", self.printable_height_mm.round() as i64);
        ctx.set_list(
            "first_layer_print_min",
            [first_layer_min.0, first_layer_min.1],
        );
        ctx.set_list(
            "first_layer_print_size",
            [first_layer_size.0, first_layer_size.1],
        );
        ctx.set_list(
            "first_layer_center_no_wipe_tower",
            [
                first_layer_min.0 + first_layer_size.0 * 0.5,
                first_layer_min.1 + first_layer_size.1 * 0.5,
            ],
        );
        ctx
    }

    /// C++ `generate_timelapse_gcode` placeholder extras on top of print config.
    pub fn placeholder_timelapse_context(
        &self,
        layer_num: usize,
        layer_z: f64,
        max_layer_z: f64,
        pos_x: i32,
        pos_y: i32,
    ) -> PlaceholderContext {
        let mut ctx = PlaceholderContext::new();
        ctx.set("layer_num", layer_num);
        ctx.set("layer_z", layer_z);
        ctx.set("max_layer_z", max_layer_z);
        ctx.set("spiral_mode", i32::from(self.spiral_mode));
        ctx.set("timelapse_type", self.timelapse_type);
        ctx.set("timelapse_inline_photo", 0);
        let has_safe = i32::from(pos_x != 0 || pos_y != 0);
        ctx.set("has_timelapse_safe_pos", has_safe);
        ctx.set("timelapse_pos_x", pos_x);
        ctx.set("timelapse_pos_y", pos_y);
        ctx.set(
            "most_used_physical_extruder_id",
            self.physical_extruder_id(0),
        );
        ctx.set("curr_physical_extruder_id", self.physical_extruder_id(0));
        // Single-object by-layer: C++ `get_is_clear_to_x0` leaves unclear_area empty.
        ctx.set("clear_to_x0", i32::from(!self.print_sequence_by_object()));
        ctx.set("print_sequence", self.print_sequence.clone());
        ctx.set(
            "farthest_point_timelapse_enabled",
            i32::from(self.farthest_point_timelapse_enabled()),
        );
        ctx
    }

    /// C++ `insert_wrapping_detection_gcode` extras on top of print config.
    pub fn placeholder_wrapping_context(
        &self,
        layer_num: usize,
        layer_z: f64,
        max_layer_z: f64,
    ) -> PlaceholderContext {
        let mut ctx = PlaceholderContext::new();
        ctx.set("layer_num", layer_num);
        ctx.set("layer_z", layer_z);
        ctx.set("max_layer_z", max_layer_z);
        ctx.set("spiral_mode", i32::from(self.spiral_mode));
        ctx.set(
            "most_used_physical_extruder_id",
            self.physical_extruder_id(0),
        );
        ctx.set("curr_physical_extruder_id", self.physical_extruder_id(0));
        ctx
    }

    /// C++ hop window: `z >= retract_lift_above && z <= retract_lift_below`.
    pub fn z_hop_in_range(&self, z_mm: f64) -> bool {
        self.z_hop_mm > 1e-9
            && z_mm + 1e-9 >= self.retract_lift_above_mm
            && z_mm <= self.retract_lift_below_mm + 1e-9
    }

    /// C++ `_travel_to_z` feed: `travel_speed_z`, else travel speed.
    pub fn z_travel_speed_mm_s(&self) -> f64 {
        if self.travel_speed_z_mm_s > 0.0 {
            self.travel_speed_z_mm_s
        } else {
            self.travel_speed_mm_s
        }
    }

    /// C++ `GCodeWriter::spiral_arc_within_bed`. Unknown bed never blocks.
    pub fn spiral_arc_within_bed(&self, center_x: f64, center_y: f64, radius: f64) -> bool {
        if !self.bed_bbox_valid {
            return true;
        }
        center_x - radius >= self.bed_min_x - 1e-9
            && center_x + radius <= self.bed_max_x + 1e-9
            && center_y - radius >= self.bed_min_y - 1e-9
            && center_y + radius <= self.bed_max_y + 1e-9
    }
}

/// C++ `int(value + 0.5)` for Marlin machine-limit lines.
fn round_machine_limit(v: f64) -> i32 {
    (v.max(0.0) + 0.5).floor() as i32
}

/// C++ `match_physical_extruder_for_each_filament`: `out[map[i]] = filaments[i]`.
fn remap_filaments_to_physical(filaments: &[i32], map: &[i32]) -> Vec<i32> {
    let n = filaments.len();
    let mut out = vec![0; n];
    for (extruder_id, &fil) in filaments.iter().enumerate() {
        let p = match map {
            [] => 0,
            m => m[extruder_id.min(m.len() - 1)],
        };
        if p >= 0 {
            let i = p as usize;
            if i < n {
                out[i] = fil;
            }
        }
    }
    out
}

/// Extrusion volume helpers (Slic3r `Flow`).
#[derive(Debug, Clone, Copy)]
pub struct Flow {
    pub width_mm: f64,
    pub height_mm: f64,
    pub filament_diameter_mm: f64,
    pub flow_ratio: f64,
    /// C++ `Flow::m_bridge`: volume is a circle of diameter [`Self::width_mm`].
    pub bridge: bool,
}

impl Flow {
    pub fn from_settings(settings: &SliceSettings, height_mm: f64) -> Self {
        Self {
            width_mm: settings.line_width_mm,
            height_mm,
            filament_diameter_mm: settings.filament_diameter_mm,
            flow_ratio: settings.flow_ratio,
            bridge: false,
        }
    }

    /// C++ `PrintRegion::flow` / `Flow::new_from_config_width` for a role.
    pub fn for_role(
        settings: &SliceSettings,
        role: FlowRole,
        height_mm: f64,
        first_layer: bool,
    ) -> Self {
        Self {
            width_mm: settings.line_width_for(role, first_layer),
            height_mm,
            filament_diameter_mm: settings.filament_diameter_mm,
            flow_ratio: settings.flow_ratio,
            bridge: false,
        }
    }

    /// C++ `LayerRegion::bridging_flow`. Thick uses a circular
    /// `sqrt(bridge_flow) * nozzle` bead; thin keeps the role width and scales
    /// volume with `bridge_flow`.
    pub fn bridging_flow(
        settings: &SliceSettings,
        role: FlowRole,
        height_mm: f64,
        first_layer: bool,
        thick: bool,
    ) -> Self {
        if thick {
            let dmr = settings.bridge_flow.max(0.0).sqrt() * settings.nozzle_diameter_mm;
            Self {
                width_mm: dmr,
                height_mm: dmr,
                filament_diameter_mm: settings.filament_diameter_mm,
                flow_ratio: 1.0,
                bridge: true,
            }
        } else {
            Self::for_role(settings, role, height_mm, first_layer)
                .with_flow_ratio(settings.bridge_flow)
        }
    }

    pub fn mm3_per_mm(self) -> f64 {
        if self.bridge {
            std::f64::consts::PI * (self.width_mm * 0.5).powi(2)
        } else {
            self.width_mm * self.height_mm * self.flow_ratio
        }
    }

    /// C++ `Flow::spacing`. Thick bridges add [`BRIDGE_EXTRA_SPACING_MM`].
    pub fn spacing_mm(self) -> f64 {
        if self.bridge {
            self.width_mm + BRIDGE_EXTRA_SPACING_MM
        } else {
            self.width_mm
        }
    }

    /// C++ `Flow::with_flow_ratio` for an extra multiplier such as `bridge_flow`.
    pub fn with_flow_ratio(self, extra: f64) -> Self {
        Self {
            flow_ratio: self.flow_ratio * extra.max(0.0),
            ..self
        }
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
