//! [`Model`] → core 3MF `3dmodel.model` XML.

use bambu_geom::TriangleMesh;
use bambu_model::{Instance, Model, ModelVolume};
use glam::Vec3;

use super::xml::{xml_escape, CORE_NS};
use crate::IoError;

pub(super) struct ModelXml {
    pub xml: String,
    pub object_ids: Vec<u32>,
    pub volume_ids: Vec<Vec<u32>>,
}

pub(super) fn model_xml_from_model(model: &Model) -> Result<ModelXml, IoError> {
    let mut ids = vec![0u32; model.objects.len()];
    let mut volume_ids = vec![Vec::new(); model.objects.len()];
    let mut next = 1u32;
    let mut resources = String::new();
    let mut build = String::new();
    for (i, obj) in model.objects.iter().enumerate() {
        let vols = obj.volumes_or_mesh();
        let world: Vec<TriangleMesh> = vols
            .iter()
            .map(|vol| instance_mesh(&vol.mesh, &obj.instances))
            .collect();
        if world.iter().all(|m| m.indices.is_empty()) {
            continue;
        }
        if vols.len() <= 1 {
            let mesh = world.into_iter().next().unwrap_or_default();
            let name = vols
                .first()
                .map(|v| v.name.as_str())
                .unwrap_or(obj.name.as_str());
            let id = next;
            next += 1;
            ids[i] = id;
            volume_ids[i] = vec![id];
            push_mesh_object(&mut resources, id, name, &mesh, vols.first());
            build.push_str(&format!("    <item objectid=\"{id}\"/>\n"));
            continue;
        }
        let mut child_ids = Vec::with_capacity(vols.len());
        for (vol, mesh) in vols.iter().zip(world.iter()) {
            if mesh.indices.is_empty() {
                child_ids.push(0);
                continue;
            }
            let id = next;
            next += 1;
            child_ids.push(id);
            push_mesh_object(&mut resources, id, &vol.name, mesh, Some(vol));
        }
        let parent = next;
        next += 1;
        ids[i] = parent;
        volume_ids[i] = child_ids.clone();
        resources.push_str(&format!(
            "    <object id=\"{parent}\" type=\"model\" name=\"{}\">\n      <components>\n",
            xml_escape(&obj.name)
        ));
        for cid in child_ids {
            if cid != 0 {
                resources.push_str(&format!(
                    "        <component objectid=\"{cid}\" transform=\"1 0 0 0 1 0 0 0 1 0 0 0\"/>\n"
                ));
            }
        }
        resources.push_str("      </components>\n    </object>\n");
        build.push_str(&format!("    <item objectid=\"{parent}\"/>\n"));
    }
    if next == 1 {
        return Err(IoError::Message("model contains no triangles".into()));
    }
    let xml = format!(
        "<?xml version=\"1.0\" encoding=\"UTF-8\"?>\n<model unit=\"millimeter\" xml:lang=\"en-US\" xmlns=\"{CORE_NS}\">\n  <resources>\n{resources}  </resources>\n  <build>\n{build}  </build>\n</model>\n"
    );
    Ok(ModelXml {
        xml,
        object_ids: ids,
        volume_ids,
    })
}

fn push_mesh_object(
    resources: &mut String,
    id: u32,
    name: &str,
    mesh: &TriangleMesh,
    vol: Option<&ModelVolume>,
) {
    resources.push_str(&format!(
        "    <object id=\"{id}\" type=\"model\" name=\"{}\">\n      <mesh>\n        <vertices>\n",
        xml_escape(name)
    ));
    for v in &mesh.vertices {
        resources.push_str(&format!(
            "          <vertex x=\"{}\" y=\"{}\" z=\"{}\"/>\n",
            v.x, v.y, v.z
        ));
    }
    resources.push_str("        </vertices>\n        <triangles>\n");
    for (i, idx) in mesh.indices.iter().enumerate() {
        resources.push_str(&format!(
            "          <triangle v1=\"{}\" v2=\"{}\" v3=\"{}\"",
            idx[0], idx[1], idx[2]
        ));
        if let Some(vol) = vol {
            push_paint_attr(
                resources,
                "paint_supports",
                vol.triangle_support.get(i).and_then(|p| p.as_hex()),
            );
            push_paint_attr(
                resources,
                "paint_seam",
                vol.triangle_seam.get(i).and_then(|p| p.as_hex()),
            );
            push_paint_attr(
                resources,
                "paint_fuzzy_skin",
                vol.triangle_fuzzy_skin.get(i).and_then(|p| p.as_hex()),
            );
            if let Some(hex) = vol.triangle_color.get(i).filter(|s| !s.is_empty()) {
                resources.push_str(&format!(" paint_color=\"{}\"", xml_escape(hex)));
            }
        }
        resources.push_str("/>\n");
    }
    resources.push_str("        </triangles>\n      </mesh>\n    </object>\n");
}

fn push_paint_attr(resources: &mut String, name: &str, hex: Option<&str>) {
    if let Some(hex) = hex {
        resources.push_str(&format!(" {name}=\"{hex}\""));
    }
}

fn instance_mesh(mesh: &TriangleMesh, instances: &[Instance]) -> TriangleMesh {
    if instances.is_empty() || (instances.len() == 1 && instances[0].offset == Vec3::ZERO) {
        return mesh.clone();
    }
    let mut out = TriangleMesh::default();
    for inst in instances {
        let mut copy = mesh.clone();
        if inst.offset != Vec3::ZERO {
            copy.translate(inst.offset);
        }
        out.append(&copy);
    }
    if out.indices.is_empty() {
        mesh.clone()
    } else {
        out
    }
}
