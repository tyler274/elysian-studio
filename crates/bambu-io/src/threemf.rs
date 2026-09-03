//! Core 3MF mesh import (`3D/3dmodel.model`) plus Bambu plates and parts.
//!
//! Geometry, units, build-item transforms, and component assemblies. When
//! `Metadata/model_settings.config` is present, object names, plates, part
//! subtype, and volume matrices are applied. `Metadata/project_settings.config`
//! carries process settings. Writers emit both files so plates, parts, and
//! settings round-trip. Paint and AMS stay ignored.

use std::collections::BTreeMap;
use std::io::{Cursor, Read, Write};
use std::path::Path;

use bambu_geom::TriangleMesh;
use bambu_model::{Instance, Model, ModelObject, ModelVolume, PartPlate};
use glam::{Mat4, Vec3, Vec4};
use quick_xml::events::Event;
use quick_xml::Reader;
use zip::write::SimpleFileOptions;
use zip::{CompressionMethod, ZipArchive, ZipWriter};

use crate::IoError;

const MODEL_PATH: &str = "3D/3dmodel.model";
const MODEL_SETTINGS_PATH: &str = "Metadata/model_settings.config";
const PROJECT_SETTINGS_PATH: &str = "Metadata/project_settings.config";
const CORE_NS: &str = "http://schemas.microsoft.com/3dmanufacturing/core/2015/02";

const CONTENT_TYPES: &str = r#"<?xml version="1.0" encoding="UTF-8"?>
<Types xmlns="http://schemas.openxmlformats.org/package/2006/content-types">
  <Default Extension="rels" ContentType="application/vnd.openxmlformats-package.relationships+xml"/>
  <Default Extension="model" ContentType="application/vnd.ms-package.3dmanufacturing-3dmodel+xml"/>
  <Default Extension="config" ContentType="application/xml"/>
</Types>
"#;

const RELS: &str = r#"<?xml version="1.0" encoding="UTF-8"?>
<Relationships xmlns="http://schemas.openxmlformats.org/package/2006/relationships">
  <Relationship Target="/3D/3dmodel.model" Id="rel-1" Type="http://schemas.microsoft.com/3dmanufacturing/2013/01/3dmodel"/>
</Relationships>
"#;

#[derive(Default)]
struct ObjectRec {
    name: String,
    vertices: Vec<Vec3>,
    triangles: Vec<[u32; 3]>,
    components: Vec<(u32, Mat4)>,
}

#[derive(Default)]
struct ParsedModel {
    unit_factor: f32,
    objects: BTreeMap<u32, ObjectRec>,
    build: Vec<(u32, Mat4)>,
    current_id: Option<u32>,
}

pub fn load_3mf(path: impl AsRef<Path>) -> Result<Model, IoError> {
    let bytes = std::fs::read(path.as_ref())?;
    load_3mf_bytes(&bytes)
}

pub fn load_3mf_bytes(bytes: &[u8]) -> Result<Model, IoError> {
    let mut zip = ZipArchive::new(Cursor::new(bytes))?;
    let entries: Vec<(String, String)> = zip
        .file_names()
        .map(|n| (n.to_string(), normalize_zip_name(n)))
        .collect();
    let original = entries
        .iter()
        .find(|(_, n)| n.eq_ignore_ascii_case(MODEL_PATH))
        .or_else(|| {
            entries.iter().find(|(_, n)| {
                let lower = n.to_ascii_lowercase();
                lower.ends_with(".model") && !lower.contains("_rels")
            })
        })
        .map(|(orig, _)| orig.clone())
        .ok_or_else(|| IoError::Message("3MF has no 3D model part".into()))?;
    let xml = zip_entry_text(&mut zip, &original)?;
    let settings_xml = zip_optional(&mut zip, &entries, MODEL_SETTINGS_PATH)?;
    let project_json = zip_optional(&mut zip, &entries, PROJECT_SETTINGS_PATH)?;
    drop(zip);
    let mut model = model_from_xml(&xml)?;
    if let Some(settings_xml) = settings_xml {
        crate::bbs::apply(&mut model, &settings_xml)?;
    }
    if let Some(project_json) = project_json {
        model.settings = Some(
            bambu_config::settings_from_json(&project_json)
                .map_err(|err| IoError::Message(err.to_string()))?,
        );
    }
    Ok(model)
}

fn zip_optional(
    zip: &mut ZipArchive<Cursor<&[u8]>>,
    entries: &[(String, String)],
    want: &str,
) -> Result<Option<String>, IoError> {
    let Some(name) = entries
        .iter()
        .find(|(_, n)| n.eq_ignore_ascii_case(want))
        .map(|(orig, _)| orig.clone())
    else {
        return Ok(None);
    };
    Ok(Some(zip_entry_text(zip, &name)?))
}

fn zip_entry_text(zip: &mut ZipArchive<Cursor<&[u8]>>, name: &str) -> Result<String, IoError> {
    let mut file = zip.by_name(name)?;
    let mut xml = String::new();
    file.read_to_string(&mut xml)?;
    Ok(xml)
}

/// Pack a single mesh as a Bambu 3MF (geometry + one plate).
pub fn write_3mf_bytes(name: &str, mesh: &TriangleMesh) -> Result<Vec<u8>, IoError> {
    write_model_3mf_bytes(&Model::from_mesh(name, mesh.clone()))
}

pub fn write_3mf(path: impl AsRef<Path>, name: &str, mesh: &TriangleMesh) -> Result<(), IoError> {
    std::fs::write(path, write_3mf_bytes(name, mesh)?)?;
    Ok(())
}

/// Pack a [`Model`] with `Metadata/model_settings.config` so plates round-trip.
pub fn write_model_3mf_bytes(model: &Model) -> Result<Vec<u8>, IoError> {
    let exported = model_xml_from_model(model)?;
    let settings = crate::bbs::write(model, &exported.object_ids, &exported.volume_ids);
    let mut cursor = Cursor::new(Vec::new());
    {
        let mut zip = ZipWriter::new(&mut cursor);
        let opts = SimpleFileOptions::default().compression_method(CompressionMethod::Deflated);
        zip.start_file("[Content_Types].xml", opts)?;
        zip.write_all(CONTENT_TYPES.as_bytes())?;
        zip.start_file("_rels/.rels", opts)?;
        zip.write_all(RELS.as_bytes())?;
        zip.start_file(MODEL_PATH, opts)?;
        zip.write_all(exported.xml.as_bytes())?;
        zip.start_file(MODEL_SETTINGS_PATH, opts)?;
        zip.write_all(settings.as_bytes())?;
        if let Some(slice) = &model.settings {
            let json = bambu_config::project_settings_json(slice)
                .map_err(|err| IoError::Message(err.to_string()))?;
            zip.start_file(PROJECT_SETTINGS_PATH, opts)?;
            zip.write_all(json.as_bytes())?;
        }
        zip.finish()?;
    }
    Ok(cursor.into_inner())
}

pub fn write_model_3mf(path: impl AsRef<Path>, model: &Model) -> Result<(), IoError> {
    std::fs::write(path, write_model_3mf_bytes(model)?)?;
    Ok(())
}

fn normalize_zip_name(name: &str) -> String {
    name.replace('\\', "/")
}

fn model_from_xml(xml: &str) -> Result<Model, IoError> {
    let mut reader = Reader::from_str(xml);
    reader.config_mut().trim_text(true);
    let mut buf = Vec::new();
    let mut parsed = ParsedModel {
        unit_factor: 1.0,
        ..ParsedModel::default()
    };

    loop {
        match reader.read_event_into(&mut buf)? {
            Event::Start(e) | Event::Empty(e) => {
                let local = e.local_name();
                match local.as_ref() {
                    b"model" => {
                        if let Some(unit) = attr(&e, b"unit") {
                            parsed.unit_factor = unit_factor(&unit);
                        }
                    }
                    b"object" => {
                        let id = attr(&e, b"id")
                            .and_then(|s| s.parse().ok())
                            .ok_or_else(|| IoError::Message("3MF object missing id".into()))?;
                        let rec = ObjectRec {
                            name: attr(&e, b"name").unwrap_or_else(|| format!("object_{id}")),
                            ..ObjectRec::default()
                        };
                        parsed.objects.insert(id, rec);
                        parsed.current_id = Some(id);
                    }
                    b"vertex" => {
                        let id = parsed
                            .current_id
                            .ok_or_else(|| IoError::Message("3MF vertex outside object".into()))?;
                        let scale = parsed.unit_factor;
                        let v = Vec3::new(
                            attr_f32(&e, b"x") * scale,
                            attr_f32(&e, b"y") * scale,
                            attr_f32(&e, b"z") * scale,
                        );
                        parsed
                            .objects
                            .get_mut(&id)
                            .ok_or_else(|| IoError::Message("3MF vertex object missing".into()))?
                            .vertices
                            .push(v);
                    }
                    b"triangle" => {
                        let id = parsed.current_id.ok_or_else(|| {
                            IoError::Message("3MF triangle outside object".into())
                        })?;
                        let tri = [
                            attr_u32(&e, b"v1"),
                            attr_u32(&e, b"v2"),
                            attr_u32(&e, b"v3"),
                        ];
                        parsed
                            .objects
                            .get_mut(&id)
                            .ok_or_else(|| IoError::Message("3MF triangle object missing".into()))?
                            .triangles
                            .push(tri);
                    }
                    b"component" => {
                        let id = parsed.current_id.ok_or_else(|| {
                            IoError::Message("3MF component outside object".into())
                        })?;
                        let child = attr(&e, b"objectid")
                            .and_then(|s| s.parse().ok())
                            .ok_or_else(|| {
                                IoError::Message("3MF component missing objectid".into())
                            })?;
                        let xf = parse_transform(attr(&e, b"transform").as_deref().unwrap_or(""));
                        parsed
                            .objects
                            .get_mut(&id)
                            .ok_or_else(|| IoError::Message("3MF component object missing".into()))?
                            .components
                            .push((child, xf));
                    }
                    b"item" => {
                        let child = attr(&e, b"objectid")
                            .and_then(|s| s.parse().ok())
                            .ok_or_else(|| {
                                IoError::Message("3MF build item missing objectid".into())
                            })?;
                        let xf = parse_transform(attr(&e, b"transform").as_deref().unwrap_or(""));
                        parsed.build.push((child, xf));
                    }
                    _ => {}
                }
            }
            Event::End(e) => {
                if e.local_name().as_ref() == b"object" {
                    parsed.current_id = None;
                }
            }
            Event::Eof => break,
            _ => {}
        }
        buf.clear();
    }

    flatten_model(parsed)
}

fn flatten_model(parsed: ParsedModel) -> Result<Model, IoError> {
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
        flatten_object(&parsed.objects, id, xf, 0, &mut leaves)?;
        let volumes: Vec<ModelVolume> = leaves
            .into_iter()
            .filter(|leaf| !leaf.mesh.indices.is_empty())
            .map(|leaf| ModelVolume::model_part(leaf.name, leaf.mesh, leaf.part_id))
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

struct LeafMesh {
    part_id: u32,
    name: String,
    mesh: TriangleMesh,
}

fn flatten_object(
    objects: &BTreeMap<u32, ObjectRec>,
    id: u32,
    xf: Mat4,
    depth: u32,
    out: &mut Vec<LeafMesh>,
) -> Result<(), IoError> {
    if depth > 32 {
        return Err(IoError::Message("3MF component recursion too deep".into()));
    }
    let obj = objects
        .get(&id)
        .ok_or_else(|| IoError::Message(format!("3MF missing object {id}")))?;
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
        });
    }
    for (child, child_xf) in &obj.components {
        flatten_object(objects, *child, xf * *child_xf, depth + 1, out)?;
    }
    Ok(())
}

/// 3MF 4×3 matrix (12 numbers) into a column-major [`Mat4`], matching C++ `get_transform_from_3mf_specs_string`.
fn parse_transform(s: &str) -> Mat4 {
    let nums: Vec<f32> = s
        .split_whitespace()
        .filter_map(|t| t.parse().ok())
        .collect();
    if nums.len() != 12 {
        return Mat4::IDENTITY;
    }
    Mat4::from_cols(
        Vec4::new(nums[0], nums[1], nums[2], 0.0),
        Vec4::new(nums[3], nums[4], nums[5], 0.0),
        Vec4::new(nums[6], nums[7], nums[8], 0.0),
        Vec4::new(nums[9], nums[10], nums[11], 1.0),
    )
}

fn unit_factor(unit: &str) -> f32 {
    match unit {
        "micron" => 0.001,
        "centimeter" => 10.0,
        "inch" => 25.4,
        "foot" => 304.8,
        "meter" => 1000.0,
        _ => 1.0,
    }
}

fn attr<'a>(e: &'a quick_xml::events::BytesStart<'a>, key: &[u8]) -> Option<String> {
    e.try_get_attribute(key)
        .ok()
        .flatten()
        .and_then(|a| String::from_utf8(a.value.into_owned()).ok())
}

fn attr_f32(e: &quick_xml::events::BytesStart<'_>, key: &[u8]) -> f32 {
    attr(e, key).and_then(|s| s.parse().ok()).unwrap_or(0.0)
}

fn attr_u32(e: &quick_xml::events::BytesStart<'_>, key: &[u8]) -> u32 {
    attr(e, key).and_then(|s| s.parse().ok()).unwrap_or(0)
}

struct ModelXml {
    xml: String,
    object_ids: Vec<u32>,
    volume_ids: Vec<Vec<u32>>,
}

fn model_xml_from_model(model: &Model) -> Result<ModelXml, IoError> {
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
            push_mesh_object(&mut resources, id, name, &mesh);
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
            push_mesh_object(&mut resources, id, &vol.name, mesh);
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

fn push_mesh_object(resources: &mut String, id: u32, name: &str, mesh: &TriangleMesh) {
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
    for idx in &mesh.indices {
        resources.push_str(&format!(
            "          <triangle v1=\"{}\" v2=\"{}\" v3=\"{}\"/>\n",
            idx[0], idx[1], idx[2]
        ));
    }
    resources.push_str("        </triangles>\n      </mesh>\n    </object>\n");
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

fn xml_escape(s: &str) -> String {
    s.replace('&', "&amp;")
        .replace('<', "&lt;")
        .replace('>', "&gt;")
        .replace('"', "&quot;")
}

#[cfg(test)]
mod tests {
    use super::*;

    fn cube_xml(unit: &str, extra_item_transform: &str) -> String {
        let item = if extra_item_transform.is_empty() {
            r#"<item objectid="1"/>"#.into()
        } else {
            format!(r#"<item objectid="1" transform="{extra_item_transform}"/>"#)
        };
        format!(
            r#"<?xml version="1.0"?>
<model unit="{unit}" xmlns="{CORE_NS}">
  <resources>
    <object id="1" name="cube" type="model">
      <mesh>
        <vertices>
          <vertex x="0" y="0" z="0"/>
          <vertex x="20" y="0" z="0"/>
          <vertex x="20" y="20" z="0"/>
          <vertex x="0" y="20" z="0"/>
          <vertex x="0" y="0" z="20"/>
          <vertex x="20" y="0" z="20"/>
          <vertex x="20" y="20" z="20"/>
          <vertex x="0" y="20" z="20"/>
        </vertices>
        <triangles>
          <triangle v1="0" v2="1" v3="2"/>
          <triangle v1="0" v2="2" v3="3"/>
          <triangle v1="4" v2="6" v3="5"/>
          <triangle v1="4" v2="7" v3="6"/>
          <triangle v1="0" v2="4" v3="5"/>
          <triangle v1="0" v2="5" v3="1"/>
          <triangle v1="2" v2="6" v3="7"/>
          <triangle v1="2" v2="7" v3="3"/>
          <triangle v1="0" v2="3" v3="7"/>
          <triangle v1="0" v2="7" v3="4"/>
          <triangle v1="1" v2="5" v3="6"/>
          <triangle v1="1" v2="6" v3="2"/>
        </triangles>
      </mesh>
    </object>
  </resources>
  <build>{item}</build>
</model>"#
        )
    }

    fn pack_xml(xml: &str) -> Vec<u8> {
        let mut cursor = Cursor::new(Vec::new());
        {
            let mut zip = ZipWriter::new(&mut cursor);
            let opts = SimpleFileOptions::default().compression_method(CompressionMethod::Stored);
            zip.start_file(MODEL_PATH, opts).unwrap();
            zip.write_all(xml.as_bytes()).unwrap();
            zip.finish().unwrap();
        }
        cursor.into_inner()
    }

    #[test]
    fn loads_cube_mesh() {
        let model = load_3mf_bytes(&pack_xml(&cube_xml("millimeter", ""))).unwrap();
        let mesh = model.merged_mesh().unwrap();
        assert_eq!(mesh.indices.len(), 12);
        let aabb = mesh.aabb().unwrap();
        assert!((aabb.size() - Vec3::splat(20.0)).length() < 1e-4);
    }

    #[test]
    fn microns_convert_to_mm() {
        let xml = cube_xml("micron", "")
            .replace("x=\"20\"", "x=\"20000\"")
            .replace("y=\"20\"", "y=\"20000\"")
            .replace("z=\"20\"", "z=\"20000\"");
        let model = load_3mf_bytes(&pack_xml(&xml)).unwrap();
        let aabb = model.merged_mesh().unwrap().aabb().unwrap();
        assert!((aabb.size() - Vec3::splat(20.0)).length() < 1e-3);
    }

    #[test]
    fn build_item_translation() {
        // Identity 3×3 plus translation (10, 0, 0).
        let xf = "1 0 0 0 1 0 0 0 1 10 0 0";
        let model = load_3mf_bytes(&pack_xml(&cube_xml("millimeter", xf))).unwrap();
        let aabb = model.merged_mesh().unwrap().aabb().unwrap();
        assert!((aabb.min.x - 10.0).abs() < 1e-4, "min.x={}", aabb.min.x);
        assert!((aabb.max.x - 30.0).abs() < 1e-4, "max.x={}", aabb.max.x);
    }

    #[test]
    fn component_assembly() {
        let xml = format!(
            r#"<?xml version="1.0"?>
<model unit="millimeter" xmlns="{CORE_NS}">
  <resources>
    <object id="1" name="part" type="model">
      <mesh>
        <vertices>
          <vertex x="0" y="0" z="0"/>
          <vertex x="1" y="0" z="0"/>
          <vertex x="0" y="1" z="0"/>
        </vertices>
        <triangles>
          <triangle v1="0" v2="1" v3="2"/>
        </triangles>
      </mesh>
    </object>
    <object id="2" name="asm" type="model">
      <components>
        <component objectid="1" transform="1 0 0 0 1 0 0 0 1 5 0 0"/>
      </components>
    </object>
  </resources>
  <build>
    <item objectid="2"/>
  </build>
</model>"#
        );
        let model = load_3mf_bytes(&pack_xml(&xml)).unwrap();
        let mesh = model.merged_mesh().unwrap();
        assert_eq!(mesh.indices.len(), 1);
        let xs: Vec<f32> = mesh.vertices.iter().map(|v| v.x).collect();
        assert!(xs.iter().any(|x| (*x - 5.0).abs() < 1e-4));
        assert!(xs.iter().any(|x| (*x - 6.0).abs() < 1e-4));
    }

    #[test]
    fn write_roundtrip_matches_cube() {
        let src = TriangleMesh::cube(20.0);
        let bytes = write_3mf_bytes("cube", &src).unwrap();
        {
            let mut zip = ZipArchive::new(Cursor::new(bytes.as_slice())).unwrap();
            assert!(
                zip.by_name(MODEL_SETTINGS_PATH).is_ok(),
                "writers emit Metadata/model_settings.config"
            );
        }
        let loaded = load_3mf_bytes(&bytes).unwrap();
        let mesh = loaded.merged_mesh().unwrap();
        assert_eq!(mesh.indices.len(), src.indices.len());
        assert_eq!(mesh.vertices.len(), src.vertices.len());
        let a = src.aabb().unwrap().size();
        let b = mesh.aabb().unwrap().size();
        assert!((a - b).length() < 1e-4);
        assert_eq!(loaded.objects[0].name, "cube");
        assert_eq!(loaded.plates.len(), 1);
        assert_eq!(loaded.plates[0].name, "Plate 1");
        assert!(loaded.settings.is_none());
    }

    fn pack_files(files: &[(&str, &str)]) -> Vec<u8> {
        let mut cursor = Cursor::new(Vec::new());
        {
            let mut zip = ZipWriter::new(&mut cursor);
            let opts = SimpleFileOptions::default().compression_method(CompressionMethod::Stored);
            for (name, body) in files {
                zip.start_file(*name, opts).unwrap();
                zip.write_all(body.as_bytes()).unwrap();
            }
            zip.finish().unwrap();
        }
        cursor.into_inner()
    }

    fn two_cubes_xml() -> String {
        format!(
            r#"<?xml version="1.0"?>
<model unit="millimeter" xmlns="{CORE_NS}">
  <resources>
    <object id="1" name="left" type="model">
      <mesh>
        <vertices>
          <vertex x="0" y="0" z="0"/>
          <vertex x="10" y="0" z="0"/>
          <vertex x="10" y="10" z="0"/>
          <vertex x="0" y="10" z="0"/>
          <vertex x="0" y="0" z="10"/>
          <vertex x="10" y="0" z="10"/>
          <vertex x="10" y="10" z="10"/>
          <vertex x="0" y="10" z="10"/>
        </vertices>
        <triangles>
          <triangle v1="0" v2="1" v3="2"/>
          <triangle v1="0" v2="2" v3="3"/>
          <triangle v1="4" v2="6" v3="5"/>
          <triangle v1="4" v2="7" v3="6"/>
          <triangle v1="0" v2="4" v3="5"/>
          <triangle v1="0" v2="5" v3="1"/>
          <triangle v1="2" v2="6" v3="7"/>
          <triangle v1="2" v2="7" v3="3"/>
          <triangle v1="0" v2="3" v3="7"/>
          <triangle v1="0" v2="7" v3="4"/>
          <triangle v1="1" v2="5" v3="6"/>
          <triangle v1="1" v2="6" v3="2"/>
        </triangles>
      </mesh>
    </object>
    <object id="2" name="right" type="model">
      <mesh>
        <vertices>
          <vertex x="0" y="0" z="0"/>
          <vertex x="10" y="0" z="0"/>
          <vertex x="10" y="10" z="0"/>
          <vertex x="0" y="10" z="0"/>
          <vertex x="0" y="0" z="10"/>
          <vertex x="10" y="0" z="10"/>
          <vertex x="10" y="10" z="10"/>
          <vertex x="0" y="10" z="10"/>
        </vertices>
        <triangles>
          <triangle v1="0" v2="1" v3="2"/>
          <triangle v1="0" v2="2" v3="3"/>
          <triangle v1="4" v2="6" v3="5"/>
          <triangle v1="4" v2="7" v3="6"/>
          <triangle v1="0" v2="4" v3="5"/>
          <triangle v1="0" v2="5" v3="1"/>
          <triangle v1="2" v2="6" v3="7"/>
          <triangle v1="2" v2="7" v3="3"/>
          <triangle v1="0" v2="3" v3="7"/>
          <triangle v1="0" v2="7" v3="4"/>
          <triangle v1="1" v2="5" v3="6"/>
          <triangle v1="1" v2="6" v3="2"/>
        </triangles>
      </mesh>
    </object>
  </resources>
  <build>
    <item objectid="1"/>
    <item objectid="2" transform="1 0 0 0 1 0 0 0 1 80 0 0"/>
  </build>
</model>"#
        )
    }

    fn two_plate_settings() -> &'static str {
        r#"<?xml version="1.0" encoding="UTF-8"?>
<config>
  <object id="1">
    <metadata key="name" value="Left cube"/>
  </object>
  <object id="2">
    <metadata key="name" value="Right cube"/>
  </object>
  <plate>
    <metadata key="plater_id" value="1"/>
    <metadata key="plater_name" value="Plate A"/>
    <metadata key="locked" value="false"/>
    <model_instance>
      <metadata key="object_id" value="1"/>
      <metadata key="instance_id" value="0"/>
    </model_instance>
  </plate>
  <plate>
    <metadata key="plater_id" value="2"/>
    <metadata key="plater_name" value="Plate B"/>
    <metadata key="locked" value="true"/>
    <model_instance>
      <metadata key="object_id" value="2"/>
      <metadata key="instance_id" value="0"/>
    </model_instance>
  </plate>
</config>"#
    }

    #[test]
    fn model_settings_names_and_plates() {
        let bytes = pack_files(&[
            (MODEL_PATH, &two_cubes_xml()),
            (MODEL_SETTINGS_PATH, two_plate_settings()),
        ]);
        let model = load_3mf_bytes(&bytes).unwrap();
        assert_eq!(model.objects.len(), 2);
        assert_eq!(model.objects[0].name, "Left cube");
        assert_eq!(model.objects[1].name, "Right cube");
        assert_eq!(model.plates.len(), 2);
        assert_eq!(model.plates[0].name, "Plate A");
        assert_eq!(model.plates[1].name, "Plate B");
        assert!(!model.plates[0].locked);
        assert!(model.plates[1].locked);
        assert_eq!(model.plates[0].object_indices, vec![0]);
        assert_eq!(model.plates[1].object_indices, vec![1]);

        let a = model.mesh_for_plate(0).unwrap().aabb().unwrap();
        let b = model.mesh_for_plate(1).unwrap().aabb().unwrap();
        assert!(
            a.max.x < 15.0,
            "plate 1 should be the untranslated cube, max.x={}",
            a.max.x
        );
        assert!(
            b.min.x > 75.0,
            "plate 2 should be the cube shifted 80 mm, min.x={}",
            b.min.x
        );
        let all = model.merged_mesh().unwrap().aabb().unwrap();
        assert!(all.max.x > 85.0);
    }

    #[test]
    fn core_3mf_without_settings_keeps_one_plate() {
        let model = load_3mf_bytes(&pack_xml(&two_cubes_xml())).unwrap();
        assert_eq!(model.plates.len(), 1);
        assert_eq!(model.plates[0].object_indices, vec![0, 1]);
        assert_eq!(model.objects[0].name, "left");
        assert_eq!(model.objects[1].name, "right");
    }

    #[test]
    fn write_model_roundtrips_plates() {
        let mut right = TriangleMesh::cube(10.0);
        right.translate(Vec3::new(80.0, 0.0, 0.0));
        let mut left_obj = ModelObject::new("Left cube", TriangleMesh::cube(10.0));
        left_obj.object_id = 7;
        left_obj.instance_id = 3;
        let mut right_obj = ModelObject::new("Right cube", right);
        right_obj.object_id = 8;
        right_obj.instance_id = 1;
        let model = Model {
            objects: vec![left_obj, right_obj],
            plates: vec![
                PartPlate {
                    name: "Plate A".into(),
                    object_indices: vec![0],
                    locked: false,
                },
                PartPlate {
                    name: "Plate B".into(),
                    object_indices: vec![1],
                    locked: true,
                },
            ],
            settings: None,
        };
        let bytes = write_model_3mf_bytes(&model).unwrap();
        let loaded = load_3mf_bytes(&bytes).unwrap();
        assert_eq!(loaded.objects.len(), 2);
        assert_eq!(loaded.objects[0].name, "Left cube");
        assert_eq!(loaded.objects[1].name, "Right cube");
        assert_eq!(loaded.plates.len(), 2);
        assert_eq!(loaded.plates[0].name, "Plate A");
        assert_eq!(loaded.plates[1].name, "Plate B");
        assert!(!loaded.plates[0].locked);
        assert!(loaded.plates[1].locked);
        assert_eq!(loaded.plates[0].object_indices, vec![0]);
        assert_eq!(loaded.plates[1].object_indices, vec![1]);
        let a = loaded.mesh_for_plate(0).unwrap().aabb().unwrap();
        let b = loaded.mesh_for_plate(1).unwrap().aabb().unwrap();
        assert!(a.max.x < 15.0, "max.x={}", a.max.x);
        assert!(b.min.x > 75.0, "min.x={}", b.min.x);
    }

    #[test]
    fn project_settings_load_from_zip() {
        let json = r#"{
            "name": "project_settings",
            "from": "project",
            "layer_height": "0.28",
            "sparse_infill_density": "15%",
            "wall_loops": "3",
            "enable_support": "1"
        }"#;
        let bytes = pack_files(&[
            (MODEL_PATH, &cube_xml("millimeter", "")),
            (PROJECT_SETTINGS_PATH, json),
        ]);
        let model = load_3mf_bytes(&bytes).unwrap();
        let s = model.settings.expect("project settings");
        assert!((s.layer_height_mm - 0.28).abs() < 1e-9);
        assert!((s.infill_density - 0.15).abs() < 1e-9);
        assert_eq!(s.wall_loops, 3);
        assert!(s.enable_support);
    }

    #[test]
    fn write_model_roundtrips_project_settings() {
        let mut model = Model::from_mesh("cube", TriangleMesh::cube(20.0));
        let mut settings = bambu_config::SliceSettings::default();
        settings.layer_height_mm = 0.16;
        settings.infill_pattern = bambu_config::InfillPattern::Grid;
        settings.wall_generator = bambu_config::WallGenerator::Arachne;
        model.settings = Some(settings.clone());
        let bytes = write_model_3mf_bytes(&model).unwrap();
        {
            let mut zip = ZipArchive::new(Cursor::new(bytes.as_slice())).unwrap();
            assert!(zip.by_name(PROJECT_SETTINGS_PATH).is_ok());
        }
        let loaded = load_3mf_bytes(&bytes).unwrap();
        let s = loaded.settings.expect("project settings");
        assert!((s.layer_height_mm - 0.16).abs() < 1e-9);
        assert_eq!(s.infill_pattern, bambu_config::InfillPattern::Grid);
        assert_eq!(s.wall_generator, bambu_config::WallGenerator::Arachne);
    }

    fn cube_mesh_xml(id: u32, name: &str, size: f32, origin: Vec3) -> String {
        let o = origin;
        let s = size;
        let v = |x, y, z| format!(r#"          <vertex x="{x}" y="{y}" z="{z}"/>"#);
        format!(
            r#"    <object id="{id}" name="{name}" type="model">
      <mesh>
        <vertices>
{v0}
{v1}
{v2}
{v3}
{v4}
{v5}
{v6}
{v7}
        </vertices>
        <triangles>
          <triangle v1="0" v2="1" v3="2"/>
          <triangle v1="0" v2="2" v3="3"/>
          <triangle v1="4" v2="6" v3="5"/>
          <triangle v1="4" v2="7" v3="6"/>
          <triangle v1="0" v2="4" v3="5"/>
          <triangle v1="0" v2="5" v3="1"/>
          <triangle v1="2" v2="6" v3="7"/>
          <triangle v1="2" v2="7" v3="3"/>
          <triangle v1="0" v2="3" v3="7"/>
          <triangle v1="0" v2="7" v3="4"/>
          <triangle v1="1" v2="5" v3="6"/>
          <triangle v1="1" v2="6" v3="2"/>
        </triangles>
      </mesh>
    </object>"#,
            v0 = v(o.x, o.y, o.z),
            v1 = v(o.x + s, o.y, o.z),
            v2 = v(o.x + s, o.y + s, o.z),
            v3 = v(o.x, o.y + s, o.z),
            v4 = v(o.x, o.y, o.z + s),
            v5 = v(o.x + s, o.y, o.z + s),
            v6 = v(o.x + s, o.y + s, o.z + s),
            v7 = v(o.x, o.y + s, o.z + s),
        )
    }

    fn negative_part_3mf() -> Vec<u8> {
        let xml = format!(
            r#"<?xml version="1.0"?>
<model unit="millimeter" xmlns="{CORE_NS}">
  <resources>
{}
{}
    <object id="3" name="cut cube" type="model">
      <components>
        <component objectid="1" transform="1 0 0 0 1 0 0 0 1 0 0 0"/>
        <component objectid="2" transform="1 0 0 0 1 0 0 0 1 0 0 0"/>
      </components>
    </object>
  </resources>
  <build>
    <item objectid="3"/>
  </build>
</model>"#,
            cube_mesh_xml(1, "body", 20.0, Vec3::ZERO),
            cube_mesh_xml(2, "cutter", 10.0, Vec3::new(5.0, 5.0, 5.0)),
        );
        let settings = r#"<?xml version="1.0" encoding="UTF-8"?>
<config>
  <object id="3">
    <metadata key="name" value="cut cube"/>
    <part id="1" subtype="normal_part">
      <metadata key="name" value="body"/>
    </part>
    <part id="2" subtype="negative_part">
      <metadata key="name" value="cutter"/>
    </part>
  </object>
</config>"#;
        pack_files(&[(MODEL_PATH, &xml), (MODEL_SETTINGS_PATH, settings)])
    }

    #[test]
    fn volume_matrix_translates_part() {
        let settings = r#"<?xml version="1.0" encoding="UTF-8"?>
<config>
  <object id="1">
    <part id="1" subtype="normal_part">
      <metadata key="matrix" value="1 0 0 10 0 1 0 0 0 0 1 0 0 0 0 1"/>
    </part>
  </object>
</config>"#;
        let bytes = pack_files(&[
            (MODEL_PATH, &cube_xml("millimeter", "")),
            (MODEL_SETTINGS_PATH, settings),
        ]);
        let model = load_3mf_bytes(&bytes).unwrap();
        let aabb = model.merged_mesh().unwrap().aabb().unwrap();
        assert!((aabb.min.x - 10.0).abs() < 1e-3, "min.x={}", aabb.min.x);
        assert!((aabb.max.x - 30.0).abs() < 1e-3, "max.x={}", aabb.max.x);
        assert_eq!(
            model.objects[0].volumes[0].volume_type,
            bambu_model::VolumeType::ModelPart
        );
    }

    #[test]
    fn negative_part_is_omitted_from_merged_mesh() {
        let model = load_3mf_bytes(&negative_part_3mf()).unwrap();
        assert_eq!(model.objects.len(), 1);
        assert_eq!(model.objects[0].volumes.len(), 2);
        assert_eq!(
            model.objects[0].volumes[0].volume_type,
            bambu_model::VolumeType::ModelPart
        );
        assert_eq!(
            model.objects[0].volumes[1].volume_type,
            bambu_model::VolumeType::Negative
        );
        let aabb = model.merged_mesh().unwrap().aabb().unwrap();
        assert!((aabb.size() - Vec3::splat(20.0)).length() < 1e-3);
        assert!(model.objects[0].volumes[1].mesh.aabb().unwrap().size().x < 11.0);
    }

    #[test]
    fn write_model_roundtrips_negative_part() {
        let mut body = ModelVolume::model_part("body", TriangleMesh::cube(20.0), 1);
        body.volume_type = bambu_model::VolumeType::ModelPart;
        let mut cutter = TriangleMesh::cube(10.0);
        cutter.translate(Vec3::new(5.0, 5.0, 5.0));
        let mut hole = ModelVolume::model_part("cutter", cutter, 2);
        hole.volume_type = bambu_model::VolumeType::Negative;
        let mut obj = ModelObject::new("cut cube", TriangleMesh::default());
        obj.volumes = vec![body, hole];
        obj.rebuild_printable_mesh();
        let model = Model {
            objects: vec![obj],
            plates: vec![PartPlate {
                name: "Plate 1".into(),
                object_indices: vec![0],
                locked: false,
            }],
            settings: None,
        };
        let bytes = write_model_3mf_bytes(&model).unwrap();
        {
            let mut zip = ZipArchive::new(Cursor::new(bytes.as_slice())).unwrap();
            let mut file = zip.by_name(MODEL_SETTINGS_PATH).unwrap();
            let mut xml = String::new();
            file.read_to_string(&mut xml).unwrap();
            assert!(xml.contains("subtype=\"negative_part\""), "{xml}");
            assert!(xml.contains("subtype=\"normal_part\""), "{xml}");
        }
        let loaded = load_3mf_bytes(&bytes).unwrap();
        assert_eq!(loaded.objects[0].volumes.len(), 2);
        let types: Vec<_> = loaded.objects[0]
            .volumes
            .iter()
            .map(|v| v.volume_type)
            .collect();
        assert!(types.contains(&bambu_model::VolumeType::ModelPart));
        assert!(types.contains(&bambu_model::VolumeType::Negative));
        let aabb = loaded.merged_mesh().unwrap().aabb().unwrap();
        assert!((aabb.size() - Vec3::splat(20.0)).length() < 1e-3);
    }

    #[test]
    fn support_modifier_parts_load_from_settings() {
        let xml = format!(
            r#"<?xml version="1.0"?>
<model unit="millimeter" xmlns="{CORE_NS}">
  <resources>
{}
{}
    <object id="3" name="painted" type="model">
      <components>
        <component objectid="1" transform="1 0 0 0 1 0 0 0 1 0 0 0"/>
        <component objectid="2" transform="1 0 0 0 1 0 0 0 1 0 0 0"/>
      </components>
    </object>
  </resources>
  <build>
    <item objectid="3"/>
  </build>
</model>"#,
            cube_mesh_xml(1, "body", 20.0, Vec3::ZERO),
            cube_mesh_xml(2, "paint", 10.0, Vec3::new(5.0, 5.0, 5.0)),
        );
        let settings = r#"<?xml version="1.0" encoding="UTF-8"?>
<config>
  <object id="3">
    <part id="1" subtype="normal_part">
      <metadata key="name" value="body"/>
    </part>
    <part id="2" subtype="support_enforcer">
      <metadata key="name" value="paint"/>
    </part>
  </object>
</config>"#;
        let model = load_3mf_bytes(&pack_files(&[
            (MODEL_PATH, &xml),
            (MODEL_SETTINGS_PATH, settings),
        ]))
        .unwrap();
        assert_eq!(model.objects[0].volumes.len(), 2);
        assert_eq!(
            model.objects[0].volumes[1].volume_type,
            bambu_model::VolumeType::SupportEnforcer
        );
        let aabb = model.merged_mesh().unwrap().aabb().unwrap();
        assert!((aabb.size() - Vec3::splat(20.0)).length() < 1e-3);
    }
}
