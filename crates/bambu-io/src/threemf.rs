//! Core 3MF mesh import (`3D/3dmodel.model`).
//!
//! Geometry, units, build-item transforms, and component assemblies. Bambu
//! extras (`Metadata/*.config`, paint, AMS) are ignored for now.

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
const CORE_NS: &str = "http://schemas.microsoft.com/3dmanufacturing/core/2015/02";

const CONTENT_TYPES: &str = r#"<?xml version="1.0" encoding="UTF-8"?>
<Types xmlns="http://schemas.openxmlformats.org/package/2006/content-types">
  <Default Extension="rels" ContentType="application/vnd.openxmlformats-package.relationships+xml"/>
  <Default Extension="model" ContentType="application/vnd.ms-package.3dmanufacturing-3dmodel+xml"/>
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
    let mut file = zip.by_name(&original)?;
    let mut xml = String::new();
    file.read_to_string(&mut xml)?;
    drop(file);
    model_from_xml(&xml)
}

/// Pack a single mesh as a core 3MF (tests and round-trips).
pub fn write_3mf_bytes(name: &str, mesh: &TriangleMesh) -> Result<Vec<u8>, IoError> {
    let xml = model_xml(name, mesh);
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
        zip.finish()?;
    }
    Ok(cursor.into_inner())
}

pub fn write_3mf(path: impl AsRef<Path>, name: &str, mesh: &TriangleMesh) -> Result<(), IoError> {
    std::fs::write(path, write_3mf_bytes(name, mesh)?)?;
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
    for (id, xf) in roots {
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

fn model_xml(name: &str, mesh: &TriangleMesh) -> String {
    let mut out = String::new();
    out.push_str("<?xml version=\"1.0\" encoding=\"UTF-8\"?>\n");
    out.push_str(&format!(
        "<model unit=\"millimeter\" xml:lang=\"en-US\" xmlns=\"{CORE_NS}\">\n"
    ));
    out.push_str("  <resources>\n");
    out.push_str(&format!(
        "    <object id=\"1\" type=\"model\" name=\"{}\">\n      <mesh>\n        <vertices>\n",
        xml_escape(name)
    ));
    for v in &mesh.vertices {
        out.push_str(&format!(
            "          <vertex x=\"{}\" y=\"{}\" z=\"{}\"/>\n",
            v.x, v.y, v.z
        ));
    }
    out.push_str("        </vertices>\n        <triangles>\n");
    for idx in &mesh.indices {
        out.push_str(&format!(
            "          <triangle v1=\"{}\" v2=\"{}\" v3=\"{}\"/>\n",
            idx[0], idx[1], idx[2]
        ));
    }
    out.push_str(
        "        </triangles>\n      </mesh>\n    </object>\n  </resources>\n  <build>\n    <item objectid=\"1\"/>\n  </build>\n</model>\n",
    );
    out
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
        let loaded = load_3mf_bytes(&bytes).unwrap();
        let mesh = loaded.merged_mesh().unwrap();
        assert_eq!(mesh.indices.len(), src.indices.len());
        assert_eq!(mesh.vertices.len(), src.vertices.len());
        let a = src.aabb().unwrap().size();
        let b = mesh.aabb().unwrap().size();
        assert!((a - b).length() < 1e-4);
        assert_eq!(loaded.objects[0].name, "cube");
    }
}
