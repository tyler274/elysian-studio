//! Component assemblies → [`Model`] volumes (one leaf mesh per 3MF object id).

use std::collections::BTreeMap;

use bambu_geom::TriangleMesh;
use bambu_model::{Instance, Model, ModelObject, ModelVolume, PartPlate, TrianglePaint};
use glam::Mat4;

use super::parse::{ObjectRec, ParsedModel};
use super::xml::normalize_model_path;
use crate::IoError;

struct LeafMesh {
    part_id: u32,
    name: String,
    mesh: TriangleMesh,
    triangle_support: Vec<TrianglePaint>,
    triangle_seam: Vec<TrianglePaint>,
    triangle_fuzzy_skin: Vec<TrianglePaint>,
    triangle_color: Vec<String>,
}

pub(super) fn flatten_files(
    files: &BTreeMap<String, ParsedModel>,
    root: &str,
) -> Result<Model, IoError> {
    let parsed = files
        .get(root)
        .ok_or_else(|| IoError::Message(format!("3MF missing root model {root:?}")))?;
    let roots: Vec<(u32, Mat4)> = if parsed.build.is_empty() {
        parsed
            .objects
            .iter()
            .filter(|(_, o)| !o.triangles.is_empty())
            .map(|(&id, _)| (id, Mat4::IDENTITY))
            .collect()
    } else {
        parsed.build.clone()
    };

    let mut objects = Vec::new();
    let mut instance_n: BTreeMap<u32, u32> = BTreeMap::new();
    for (id, xf) in roots {
        let instance_id = *instance_n.entry(id).and_modify(|n| *n += 1).or_insert(0);
        let mut leaves = Vec::new();
        flatten_object(files, root, id, xf, 0, &mut leaves)?;
        let volumes: Vec<ModelVolume> = leaves
            .into_iter()
            .filter(|leaf| !leaf.mesh.indices.is_empty())
            .map(|leaf| {
                let mut vol = ModelVolume::model_part(leaf.name, leaf.mesh, leaf.part_id);
                vol.triangle_support = leaf.triangle_support;
                vol.triangle_seam = leaf.triangle_seam;
                vol.triangle_fuzzy_skin = leaf.triangle_fuzzy_skin;
                vol.triangle_color = leaf.triangle_color;
                vol
            })
            .collect();
        if volumes.is_empty() {
            continue;
        }
        let name = parsed
            .objects
            .get(&id)
            .map(|o| o.name.clone())
            .filter(|s| !s.is_empty())
            .unwrap_or_else(|| format!("object_{id}"));
        let mut obj = ModelObject {
            name,
            mesh: TriangleMesh::default(),
            volumes,
            instances: vec![Instance::default()],
            object_id: id,
            instance_id,
        };
        obj.rebuild_printable_mesh();
        objects.push(obj);
    }
    if objects.is_empty() {
        return Err(IoError::Message("3MF contains no triangles".into()));
    }
    let n = objects.len();
    Ok(Model {
        objects,
        plates: vec![PartPlate {
            name: "Plate 1".into(),
            object_indices: (0..n).collect(),
            locked: false,
        }],
        settings: None,
    })
}

fn object_rec<'a>(
    files: &'a BTreeMap<String, ParsedModel>,
    file: &str,
    id: u32,
) -> Result<&'a ObjectRec, IoError> {
    let parsed = files.get(file).ok_or_else(|| {
        IoError::Message(format!("3MF missing model part {file:?} (object {id})"))
    })?;
    parsed
        .objects
        .get(&id)
        .ok_or_else(|| IoError::Message(format!("3MF missing object {id} in {file:?}")))
}

fn flatten_object(
    files: &BTreeMap<String, ParsedModel>,
    file: &str,
    id: u32,
    xf: Mat4,
    depth: u32,
    out: &mut Vec<LeafMesh>,
) -> Result<(), IoError> {
    if depth > 32 {
        return Err(IoError::Message("3MF component recursion too deep".into()));
    }
    let obj = object_rec(files, file, id)?;
    if !obj.triangles.is_empty() {
        let vertices = obj
            .vertices
            .iter()
            .map(|p| xf.transform_point3(*p))
            .collect();
        out.push(LeafMesh {
            part_id: id,
            name: obj.name.clone(),
            mesh: TriangleMesh {
                vertices,
                indices: obj.triangles.clone(),
            },
            triangle_support: obj.triangle_support.clone(),
            triangle_seam: obj.triangle_seam.clone(),
            triangle_fuzzy_skin: obj.triangle_fuzzy_skin.clone(),
            triangle_color: obj.triangle_color.clone(),
        });
    }
    let components = obj.components.clone();
    for (child_path, child, child_xf) in components {
        let next_file = if child_path.is_empty() {
            file.to_string()
        } else {
            let want = normalize_model_path(&child_path);
            if files.contains_key(&want) {
                want
            } else {
                return Err(IoError::Message(format!(
                    "3MF component p:path {child_path:?} not in package (object {child})"
                )));
            }
        };
        flatten_object(files, &next_file, child, xf * child_xf, depth + 1, out)?;
    }
    Ok(())
}
