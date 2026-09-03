#![forbid(unsafe_code)]

use std::collections::BTreeMap;

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

    pub fn is_modifier(self) -> bool {
        matches!(self, Self::Modifier)
    }

    pub fn is_model_part(self) -> bool {
        matches!(self, Self::ModelPart)
    }

    pub fn is_negative(self) -> bool {
        matches!(self, Self::Negative)
    }

    pub fn is_support_enforcer(self) -> bool {
        matches!(self, Self::SupportEnforcer)
    }

    pub fn is_support_blocker(self) -> bool {
        matches!(self, Self::SupportBlocker)
    }

    pub fn is_support_modifier(self) -> bool {
        self.is_support_enforcer() || self.is_support_blocker()
    }
}

/// Per-triangle `paint_supports` (C++ `EnforcerBlockerType` on `supported_facets`).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum TrianglePaint {
    #[default]
    None,
    Enforcer,
    Blocker,
}

impl TrianglePaint {
    /// Decode Bambu/Prusa `TriangleSelector::serialize` hex (`4` enforcer, `8` blocker).
    pub fn from_hex(s: &str) -> Self {
        if s.is_empty() {
            return Self::None;
        }
        let bits = hex_to_bits(s);
        let mut i = 0;
        decode_paint_node(&bits, &mut i)
    }

    pub fn from_support_hex(s: &str) -> Self {
        Self::from_hex(s)
    }

    pub fn as_hex(self) -> Option<&'static str> {
        match self {
            Self::None => None,
            Self::Enforcer => Some("4"),
            Self::Blocker => Some("8"),
        }
    }

    pub fn as_support_hex(self) -> Option<&'static str> {
        self.as_hex()
    }

    fn or_paint(self, other: Self) -> Self {
        match (self, other) {
            (Self::Enforcer, _) | (_, Self::Enforcer) => Self::Enforcer,
            (Self::Blocker, _) | (_, Self::Blocker) => Self::Blocker,
            _ => Self::None,
        }
    }
}

fn hex_to_bits(s: &str) -> Vec<bool> {
    let mut bits = Vec::with_capacity(s.len() * 4);
    for ch in s.chars().rev() {
        let dec = match ch {
            '0'..='9' => ch as u32 - '0' as u32,
            'A'..='F' => 10 + ch as u32 - 'A' as u32,
            'a'..='f' => 10 + ch as u32 - 'a' as u32,
            _ => continue,
        };
        for i in 0..4 {
            bits.push((dec & (1 << i)) != 0);
        }
    }
    bits
}

fn decode_paint_node(bits: &[bool], i: &mut usize) -> TrianglePaint {
    if *i + 4 > bits.len() {
        return TrianglePaint::None;
    }
    let split = u8::from(bits[*i]) | (u8::from(bits[*i + 1]) << 1);
    *i += 2;
    if split == 0 {
        if *i + 2 > bits.len() {
            return TrianglePaint::None;
        }
        let n = u8::from(bits[*i]) | (u8::from(bits[*i + 1]) << 1);
        *i += 2;
        if n == 3 {
            while *i + 4 <= bits.len() {
                *i += 4;
            }
            return TrianglePaint::None;
        }
        return match n {
            1 => TrianglePaint::Enforcer,
            2 => TrianglePaint::Blocker,
            _ => TrianglePaint::None,
        };
    }
    *i += 2;
    let children = usize::from(split) + 1;
    let mut acc = TrianglePaint::None;
    for _ in 0..children {
        acc = acc.or_paint(decode_paint_node(bits, i));
    }
    acc
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
    /// `paint_supports` on each triangle (`indices` order). Empty means none.
    pub triangle_support: Vec<TrianglePaint>,
    /// `paint_seam` (`Enforcer` = forced seam, `Blocker` = avoid).
    pub triangle_seam: Vec<TrianglePaint>,
    /// `paint_fuzzy_skin` (`Enforcer`/`Blocker` both mean painted fuzzy).
    pub triangle_fuzzy_skin: Vec<TrianglePaint>,
    /// Raw `paint_color` hex per triangle (MMU; ignored at slice time).
    pub triangle_color: Vec<String>,
    /// Extra `<metadata key>` on this part (`volume.config` in C++).
    pub config: BTreeMap<String, String>,
}

impl ModelVolume {
    pub fn model_part(name: impl Into<String>, mesh: TriangleMesh, part_id: u32) -> Self {
        Self {
            name: name.into(),
            mesh,
            volume_type: VolumeType::ModelPart,
            part_id,
            matrix: Mat4::IDENTITY,
            triangle_support: Vec::new(),
            triangle_seam: Vec::new(),
            triangle_fuzzy_skin: Vec::new(),
            triangle_color: Vec::new(),
            config: BTreeMap::new(),
        }
    }

    pub fn has_support_paint(&self) -> bool {
        self.triangle_support
            .iter()
            .any(|p| *p != TrianglePaint::None)
    }

    pub fn has_seam_paint(&self) -> bool {
        self.triangle_seam.iter().any(|p| *p != TrianglePaint::None)
    }

    pub fn has_fuzzy_paint(&self) -> bool {
        self.triangle_fuzzy_skin
            .iter()
            .any(|p| *p != TrianglePaint::None)
    }

    pub fn has_color_paint(&self) -> bool {
        self.triangle_color.iter().any(|s| !s.is_empty())
    }

    pub fn has_region_config(&self) -> bool {
        self.config.keys().any(|k| bambu_config::is_region_key(k))
    }

    /// Parent region settings with this volume's PrintRegion keys applied.
    pub fn region_settings(&self, parent: &SliceSettings) -> SliceSettings {
        let mut out = parent.clone();
        bambu_config::apply_config_pairs(&mut out, &self.config, true);
        out
    }

    /// CPU Clipper path: negatives, support modifiers, paint, or region overrides.
    pub fn needs_volume_slice(&self) -> bool {
        self.volume_type.is_negative()
            || self.volume_type.is_support_modifier()
            || self.has_support_paint()
            || self.has_seam_paint()
            || self.has_fuzzy_paint()
            || (self.volume_type.is_modifier() && self.has_region_config())
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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn paint_supports_hex_unsplit() {
        assert_eq!(TrianglePaint::from_hex(""), TrianglePaint::None);
        assert_eq!(TrianglePaint::from_hex("4"), TrianglePaint::Enforcer);
        assert_eq!(TrianglePaint::from_hex("8"), TrianglePaint::Blocker);
        assert_eq!(TrianglePaint::Enforcer.as_hex(), Some("4"));
        assert_eq!(TrianglePaint::Blocker.as_hex(), Some("8"));
    }

    #[test]
    fn region_config_overlays_infill() {
        let mut vol = ModelVolume::model_part("mod", TriangleMesh::default(), 2);
        vol.volume_type = VolumeType::Modifier;
        vol.config
            .insert("sparse_infill_density".into(), "100%".into());
        vol.config.insert("layer_height".into(), "0.08".into());
        assert!(vol.has_region_config());
        assert!(vol.needs_volume_slice());
        let parent = SliceSettings::default();
        let over = vol.region_settings(&parent);
        assert!((over.infill_density - 1.0).abs() < 1e-9);
        assert!((over.layer_height_mm - parent.layer_height_mm).abs() < 1e-9);
    }
}
