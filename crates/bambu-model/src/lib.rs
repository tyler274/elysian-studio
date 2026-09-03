#![forbid(unsafe_code)]

use bambu_config::SliceSettings;
use bambu_geom::TriangleMesh;
use glam::Vec3;
use serde::{Deserialize, Serialize};

#[derive(Debug, Clone)]
pub struct Model {
    pub objects: Vec<ModelObject>,
    pub plates: Vec<PartPlate>,
}

#[derive(Debug, Clone)]
pub struct ModelObject {
    pub name: String,
    pub mesh: TriangleMesh,
    pub instances: Vec<Instance>,
}

#[derive(Debug, Clone, Copy)]
pub struct Instance {
    pub offset: Vec3,
}

impl Default for Instance {
    fn default() -> Self {
        Self {
            offset: Vec3::ZERO,
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PartPlate {
    pub name: String,
    pub object_indices: Vec<usize>,
}

impl Model {
    pub fn from_mesh(name: impl Into<String>, mesh: TriangleMesh) -> Self {
        Self {
            objects: vec![ModelObject {
                name: name.into(),
                mesh,
                instances: vec![Instance::default()],
            }],
            plates: vec![PartPlate {
                name: "Plate 1".into(),
                object_indices: vec![0],
            }],
        }
    }

    pub fn first_mesh(&self) -> Option<&TriangleMesh> {
        self.objects.first().map(|o| &o.mesh)
    }
}

pub fn default_settings() -> SliceSettings {
    SliceSettings::default()
}
