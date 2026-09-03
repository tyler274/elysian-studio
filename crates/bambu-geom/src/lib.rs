//! Scaled integer geometry, Clipper2 wrappers, and triangle meshes.
//!
//! Coordinates use Slic3r's `SCALING_FACTOR` (1e6) so polygon booleans stay
//! on integers.

#![forbid(unsafe_code)]

mod clipper;
mod mesh;
mod point;

pub use clipper::{offset_polygons, union_polygons};
pub use mesh::{Aabb3, TriangleMesh};
pub use point::{
    scale, unscale, Point, Polygon, Polyline, SCALING_FACTOR, SCALING_FACTOR_F64,
};

pub type Polygons = Vec<Polygon>;
