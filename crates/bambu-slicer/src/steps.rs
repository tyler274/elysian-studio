//! Print step identifiers matching C++ `Print.hpp`.

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
