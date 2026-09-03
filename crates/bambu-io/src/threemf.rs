//! Core 3MF mesh import (`3D/3dmodel.model`) plus Bambu plates.
//!
//! Geometry, units, build-item transforms, and component assemblies. When
//! `Metadata/model_settings.config` is present, object names and plate
//! grouping are applied. Writers emit that file so plates round-trip. Paint,
//! AMS, and volume matrices stay ignored.

use std::collections::BTreeMap;
use std::io::{Cursor, Read, Write};
use std::path::Path;

use bambu_geom::TriangleMesh;
use bambu_model::{Instance, Model, ModelObject, PartPlate};
use glam::{Mat4, Vec3, Vec4};
use quick_xml::events::Event;
use quick_xml::Reader;
use zip::write::SimpleFileOptions;
use zip::{CompressionMethod, ZipArchive, ZipWriter};

use crate::IoError;

const MODEL_PATH: &str = "3D/3dmodel.model";
const MODEL_SETTINGS_PATH: &str = "Metadata/model_settings.config";
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
    let settings = entries
        .iter()
        .find(|(_, n)| n.eq_ignore_ascii_case(MODEL_SETTINGS_PATH))
        .map(|(orig, _)| orig.clone());
    let settings_xml = match settings {
        Some(name) => Some(zip_entry_text(&mut zip, &name)?),
        None => None,
    };
    drop(zip);
    let mut model = model_from_xml(&xml)?;
    if let Some(settings_xml) = settings_xml {
        crate::bbs::apply(&mut model, &settings_xml)?;
    }
    Ok(model)
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
    let (xml, ids) = model_xml_from_model(model)?;
    let settings = crate::bbs::write(model, &ids);
    let mut cursor = Cursor::new(Vec::new());
    {
        let mut zip = ZipWriter::new(&mut cursor);
        let opts = SimpleFileOptions::default().compression_method(CompressionMethod::Deflated);
        zip.start_file("[Content_Types].xml", opts)?;
        zip.write_all(CONTENT_TYPES.as_bytes())?;
        zip.start_file("_rels/.rels", opts)?;
        zip.write_all(RELS.as_bytes())?;
        zip.start_file(MODEL_PATH, opts)?;
        zip.write_all(xml.as_bytes())?;
        zip.start_file(MODEL_SETTINGS_PATH, opts)?;
        zip.write_all(settings.as_bytes())?;
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
        let mut meshes = Vec::new();
        flatten_object(&parsed.objects, id, xf, 0, &mut meshes)?;
        for (name, mesh) in meshes {
            if mesh.indices.is_empty() {
                continue;
            }
            objects.push(ModelObject {
                name,
                mesh,
                instances: vec![Instance::default()],
                object_id: id,
                instance_id,
            });
        }
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
    })
}

fn flatten_object(
    objects: &BTreeMap<u32, ObjectRec>,
    id: u32,
    xf: Mat4,
    depth: u32,
    out: &mut Vec<(String, TriangleMesh)>,
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
        out.push((
            obj.name.clone(),
            TriangleMesh {
                vertices,
                indices: obj.triangles.clone(),
            },
        ));
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

fn model_xml_from_model(model: &Model) -> Result<(String, Vec<u32>), IoError> {
    let mut ids = vec![0u32; model.objects.len()];
    let mut next = 1u32;
    let mut resources = String::new();
    let mut build = String::new();
    for (i, obj) in model.objects.iter().enumerate() {
        let mesh = world_mesh(obj);
        if mesh.indices.is_empty() {
            continue;
        }
        let id = next;
        next += 1;
        ids[i] = id;
        resources.push_str(&format!(
            "    <object id=\"{id}\" type=\"model\" name=\"{}\">\n      <mesh>\n        <vertices>\n",
            xml_escape(&obj.name)
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
        build.push_str(&format!("    <item objectid=\"{id}\"/>\n"));
    }
    if next == 1 {
        return Err(IoError::Message("model contains no triangles".into()));
    }
    let xml = format!(
        "<?xml version=\"1.0\" encoding=\"UTF-8\"?>\n<model unit=\"millimeter\" xml:lang=\"en-US\" xmlns=\"{CORE_NS}\">\n  <resources>\n{resources}  </resources>\n  <build>\n{build}  </build>\n</model>\n"
    );
    Ok((xml, ids))
}

fn world_mesh(obj: &ModelObject) -> TriangleMesh {
    if obj.instances.is_empty() {
        return obj.mesh.clone();
    }
    if obj.instances.len() == 1 && obj.instances[0].offset == Vec3::ZERO {
        return obj.mesh.clone();
    }
    let mut out = TriangleMesh::default();
    for inst in &obj.instances {
        let mut mesh = obj.mesh.clone();
        if inst.offset != Vec3::ZERO {
            mesh.translate(inst.offset);
        }
        out.append(&mesh);
    }
    if out.indices.is_empty() {
        obj.mesh.clone()
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
        let model = Model {
            objects: vec![
                ModelObject {
                    name: "Left cube".into(),
                    mesh: TriangleMesh::cube(10.0),
                    instances: vec![Instance::default()],
                    object_id: 7,
                    instance_id: 3,
                },
                ModelObject {
                    name: "Right cube".into(),
                    mesh: right,
                    instances: vec![Instance::default()],
                    object_id: 8,
                    instance_id: 1,
                },
            ],
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
}
