//! Scaled integer geometry, Clipper2 wrappers, and triangle meshes.
//!
//! Coordinates use Slic3r's `SCALING_FACTOR` (1e6) so polygon booleans stay
//! on integers.

#![forbid(unsafe_code)]

mod arc_fitter;
mod clipper;
mod mesh;
mod point;

pub use arc_fitter::{
    calc_arc_length_mm, douglas_peucker, fit_arcs_and_simplify, ArcDir, ArcSegment, PathFit,
    PathFitKind,
};
pub use clipper::{
    difference_polygons, intersect_polygons, offset_polygons, offset_polygons_square,
    union_polygons,
};
pub use mesh::{Aabb3, TriangleMesh};
pub use point::{
    clip_end, scale, unscale, Point, Polygon, Polyline, SCALING_FACTOR, SCALING_FACTOR_F64,
};

pub type Polygons = Vec<Polygon>;
