#![forbid(unsafe_code)]

use bambu_config::SliceSettings;
use bambu_geom::TriangleMesh;
use glam::{Mat4, Vec3};
use serde::{Deserialize, Serialize};

#[derive(Debug, Clone)]
pub struct Model {
    pub objects: Vec<ModelObject>,
    pub plates: Vec<PartPlate>,
    /// Process settings from `Metadata/project_settings.config`, if present.
    pub settings: Option<SliceSettings>,
}

/// Bambu `subtype` on `<part>` / `<volume>` in `model_settings.config`.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum VolumeType {
    #[default]
    ModelPart,
    Negative,
    Modifier,
    SupportEnforcer,
    SupportBlocker,
}

impl VolumeType {
    pub fn from_subtype(s: &str) -> Self {
        match s {
            "negative_part" | "NegativeVolume" => Self::Negative,
            "modifier_part" | "ParameterModifier" => Self::Modifier,
            "support_enforcer" | "SupportEnforcer" => Self::SupportEnforcer,
            "support_blocker" | "SupportBlocker" => Self::SupportBlocker,
            _ => Self::ModelPart,
        }
    }

    pub fn as_str(self) -> &'static str {
        match self {
            Self::ModelPart => "normal_part",
            Self::Negative => "negative_part",
            Self::Modifier => "modifier_part",
            Self::SupportEnforcer => "support_enforcer",
            Self::SupportBlocker => "support_blocker",
        }
    }

    pub fn is_model_part(self) -> bool {
        matches!(self, Self::ModelPart)
    }

    pub fn is_negative(self) -> bool {
        matches!(self, Self::Negative)
    }
}

/// One mesh of a [`ModelObject`] (C++ `ModelVolume`).
#[derive(Debug, Clone)]
pub struct ModelVolume {
    pub name: String,
    pub mesh: TriangleMesh,
    pub volume_type: VolumeType,
    /// 3MF resource id of this part (`<part id>`).
    pub part_id: u32,
    /// Extra 4×4 from `metadata key="matrix"` (identity if none).
    pub matrix: Mat4,
}

impl ModelVolume {
    pub fn model_part(name: impl Into<String>, mesh: TriangleMesh, part_id: u32) -> Self {
        Self {
            name: name.into(),
            mesh,
            volume_type: VolumeType::ModelPart,
            part_id,
            matrix: Mat4::IDENTITY,
        }
    }
}

#[derive(Debug, Clone)]
pub struct ModelObject {
    pub name: String,
    /// Concatenated [`VolumeType::ModelPart`] meshes (for AABB / GPU / STL).
    pub mesh: TriangleMesh,
    pub volumes: Vec<ModelVolume>,
    pub instances: Vec<Instance>,
    /// 3MF resource id (1 for STL / single-mesh projects).
    pub object_id: u32,
    /// 0-based instance among build items that share [`Self::object_id`].
    pub instance_id: u32,
}

impl ModelObject {
    pub fn new(name: impl Into<String>, mesh: TriangleMesh) -> Self {
        let name = name.into();
        Self {
            volumes: vec![ModelVolume::model_part(name.clone(), mesh.clone(), 1)],
            name,
            mesh,
            instances: vec![Instance::default()],
            object_id: 1,
            instance_id: 0,
        }
    }

    /// Volumes if present, otherwise a single model-part wrapping [`Self::mesh`].
    pub fn volumes_or_mesh(&self) -> Vec<ModelVolume> {
        if self.volumes.is_empty() {
            vec![ModelVolume::model_part(
                self.name.clone(),
                self.mesh.clone(),
                self.object_id.max(1),
            )]
        } else {
            self.volumes.clone()
        }
    }

    /// Union of model-part meshes only (negatives / modifiers omitted).
    pub fn printable_mesh(&self) -> TriangleMesh {
        let volumes = self.volumes_or_mesh();
        let mut out = TriangleMesh::default();
        for vol in &volumes {
            if vol.volume_type.is_model_part() {
                out.append(&vol.mesh);
            }
        }
        out
    }

    pub fn rebuild_printable_mesh(&mut self) {
        self.mesh = self.printable_mesh();
    }
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
            objects: vec![ModelObject::new(name, mesh)],
            plates: vec![PartPlate {
                name: "Plate 1".into(),
                object_indices: vec![0],
                locked: false,
            }],
            settings: None,
        }
    }

    pub fn first_mesh(&self) -> Option<&TriangleMesh> {
        self.objects.first().map(|o| &o.mesh)
    }

    /// Concatenate printable meshes after applying instance offsets.
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

    /// World-space volumes on `plate` (instance offsets baked into each mesh).
    pub fn world_volumes_for_plate(&self, plate: usize) -> Vec<ModelVolume> {
        let indices: Vec<usize> = match self.plates.get(plate) {
            Some(p) => p.object_indices.clone(),
            None => (0..self.objects.len()).collect(),
        };
        let mut out = Vec::new();
        for i in indices {
            let Some(object) = self.objects.get(i) else {
                continue;
            };
            let volumes = object.volumes_or_mesh();
            let instances = if object.instances.is_empty() {
                vec![Instance::default()]
            } else {
                object.instances.clone()
            };
            for inst in instances {
                for mut vol in volumes.clone() {
                    if inst.offset != Vec3::ZERO {
                        vol.mesh.translate(inst.offset);
                    }
                    out.push(vol);
                }
            }
        }
        out
    }

    fn merge_indices(&self, indices: impl IntoIterator<Item = usize>) -> Option<TriangleMesh> {
        let mut out = TriangleMesh::default();
        for i in indices {
            let Some(object) = self.objects.get(i) else {
                continue;
            };
            let base = object.printable_mesh();
            if object.instances.is_empty() {
                out.append(&base);
                continue;
            }
            for inst in &object.instances {
                let mut mesh = base.clone();
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
