//! Print step identifiers.
//!
//! Bambu `Print.hpp` plus PrusaSlicer 3.0 `FDMPrintStep` / `FDMPrintObjectStep`
//! (`PrintSteps.hpp`). Wipe tower is still a print-level step (synonym of tool
//! ordering in PS 3.0). Object steps stay CPU clipper work.
//!
//! PS 3.0 adds print-level `AlertWhenSupportsNeeded` and object-level
//! `SupportSpotsSearch`, `EstimateCurledExtrusions`, and
//! `CalculateOverhangingPerimeters` (not run yet). Bambu keeps
//! `DetectOverhangsForLift` and wall/infill/support simplify steps.

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PrintStep {
    WipeTower,
    SkirtBrim,
    GCodeExport,
    ConflictCheck,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PrintObjectStep {
    Slice,
    Perimeters,
    PrepareInfill,
    Infill,
    Ironing,
    SupportMaterial,
    DetectOverhangsForLift,
    SimplifyWall,
    SimplifyInfill,
    SimplifySupportPath,
}
