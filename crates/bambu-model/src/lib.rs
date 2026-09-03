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
    /// 3MF resource id (1 for STL / single-mesh projects).
    pub object_id: u32,
    /// 0-based instance among build items that share [`Self::object_id`].
    pub instance_id: u32,
}

#[derive(Debug, Clone, Copy)]
pub struct Instance {
    pub offset: Vec3,
}

impl Default for Instance {
    fn default() -> Self {
        Self { offset: Vec3::ZERO }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PartPlate {
    pub name: String,
    pub object_indices: Vec<usize>,
    pub locked: bool,
}

impl Model {
    pub fn from_mesh(name: impl Into<String>, mesh: TriangleMesh) -> Self {
        Self {
            objects: vec![ModelObject {
                name: name.into(),
                mesh,
                instances: vec![Instance::default()],
                object_id: 1,
                instance_id: 0,
            }],
            plates: vec![PartPlate {
                name: "Plate 1".into(),
                object_indices: vec![0],
                locked: false,
            }],
        }
    }

    pub fn first_mesh(&self) -> Option<&TriangleMesh> {
        self.objects.first().map(|o| &o.mesh)
    }

    /// Concatenate every object mesh after applying instance offsets.
    pub fn merged_mesh(&self) -> Option<TriangleMesh> {
        self.merge_indices(0..self.objects.len())
    }

    /// Objects on `plate` (0-based). Missing plate falls back to [`Self::merged_mesh`].
    pub fn mesh_for_plate(&self, plate: usize) -> Option<TriangleMesh> {
        let Some(p) = self.plates.get(plate) else {
            return self.merged_mesh();
        };
        self.merge_indices(p.object_indices.iter().copied())
    }

    fn merge_indices(&self, indices: impl IntoIterator<Item = usize>) -> Option<TriangleMesh> {
        let mut out = TriangleMesh::default();
        for i in indices {
            let Some(object) = self.objects.get(i) else {
                continue;
            };
            if object.instances.is_empty() {
                out.append(&object.mesh);
                continue;
            }
            for inst in &object.instances {
                let mut mesh = object.mesh.clone();
                if inst.offset != Vec3::ZERO {
                    mesh.translate(inst.offset);
                }
                out.append(&mesh);
            }
        }
        if out.indices.is_empty() {
            None
        } else {
            Some(out)
        }
    }
}

pub fn default_settings() -> SliceSettings {
    SliceSettings::default()
}
