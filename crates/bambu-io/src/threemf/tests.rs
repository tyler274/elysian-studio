use std::io::{Cursor, Read, Write};

use bambu_geom::TriangleMesh;
use bambu_model::{Model, ModelObject, ModelVolume, PartPlate, TrianglePaint};
use glam::Vec3;
use zip::write::SimpleFileOptions;
use zip::{CompressionMethod, ZipArchive, ZipWriter};

use super::{
    load_3mf_bytes, write_3mf_bytes, write_model_3mf_bytes, CORE_NS, MODEL_PATH,
    MODEL_SETTINGS_PATH, PROJECT_SETTINGS_PATH,
};

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
fn production_ppath_loads_nested_model() {
    let child = format!(
        r#"<?xml version="1.0"?>
<model unit="millimeter" xmlns="{CORE_NS}">
  <resources>
{}
  </resources>
  <build></build>
</model>"#,
        cube_mesh_xml(1, "part", 10.0, Vec3::ZERO),
    );
    let parent = format!(
        r#"<?xml version="1.0"?>
<model unit="millimeter" xmlns="{CORE_NS}" xmlns:p="http://schemas.microsoft.com/3dmanufacturing/production/2015/06">
  <resources>
<object id="9" name="asm" type="model">
  <components>
    <component p:path="/3D/Objects/object_2.model" objectid="1" transform="1 0 0 0 1 0 0 0 1 5 0 0"/>
  </components>
</object>
  </resources>
  <build>
<item objectid="9"/>
  </build>
</model>"#
    );
    let bytes = pack_files(&[(MODEL_PATH, &parent), ("3D/Objects/object_2.model", &child)]);
    let model = load_3mf_bytes(&bytes).unwrap();
    assert_eq!(model.objects.len(), 1);
    assert_eq!(model.objects[0].volumes.len(), 1);
    let mesh = model.merged_mesh().unwrap();
    assert_eq!(mesh.indices.len(), 12);
    let aabb = mesh.aabb().unwrap();
    assert!((aabb.min.x - 5.0).abs() < 1e-3, "min.x={}", aabb.min.x);
    assert!((aabb.max.x - 15.0).abs() < 1e-3, "max.x={}", aabb.max.x);
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

#[test]
fn paint_supports_load_and_roundtrip() {
    let mut xml = cube_xml("millimeter", "");
    xml = xml.replacen(
        r#"<triangle v1="0" v2="1" v3="2"/>"#,
        r#"<triangle v1="0" v2="1" v3="2" paint_supports="4"/>"#,
        1,
    );
    let model = load_3mf_bytes(&pack_xml(&xml)).unwrap();
    assert_eq!(
        model.objects[0].volumes[0].triangle_support[0],
        TrianglePaint::Enforcer
    );
    let bytes = write_model_3mf_bytes(&model).unwrap();
    {
        let mut zip = ZipArchive::new(Cursor::new(bytes.as_slice())).unwrap();
        let mut file = zip.by_name(MODEL_PATH).unwrap();
        let mut body = String::new();
        file.read_to_string(&mut body).unwrap();
        assert!(body.contains("paint_supports=\"4\""), "{body}");
    }
    let loaded = load_3mf_bytes(&bytes).unwrap();
    assert!(loaded.objects[0].volumes[0]
        .triangle_support
        .iter()
        .any(|p| *p == TrianglePaint::Enforcer));
}

#[test]
fn paint_seam_fuzzy_color_roundtrip() {
    let mut xml = cube_xml("millimeter", "");
    xml = xml.replacen(
        r#"<triangle v1="0" v2="1" v3="2"/>"#,
        r#"<triangle v1="0" v2="1" v3="2" paint_seam="4" paint_fuzzy_skin="8" paint_color="1"/>"#,
        1,
    );
    let model = load_3mf_bytes(&pack_xml(&xml)).unwrap();
    let vol = &model.objects[0].volumes[0];
    assert_eq!(vol.triangle_seam[0], TrianglePaint::Enforcer);
    assert_eq!(vol.triangle_fuzzy_skin[0], TrianglePaint::Blocker);
    assert_eq!(vol.triangle_color[0], "1");
    let bytes = write_model_3mf_bytes(&model).unwrap();
    {
        let mut zip = ZipArchive::new(Cursor::new(bytes.as_slice())).unwrap();
        let mut file = zip.by_name(MODEL_PATH).unwrap();
        let mut body = String::new();
        file.read_to_string(&mut body).unwrap();
        assert!(body.contains("paint_seam=\"4\""), "{body}");
        assert!(body.contains("paint_fuzzy_skin=\"8\""), "{body}");
        assert!(body.contains("paint_color=\"1\""), "{body}");
    }
    let loaded = load_3mf_bytes(&bytes).unwrap();
    let vol = &loaded.objects[0].volumes[0];
    assert!(vol
        .triangle_seam
        .iter()
        .any(|p| *p == TrianglePaint::Enforcer));
    assert!(vol
        .triangle_fuzzy_skin
        .iter()
        .any(|p| *p == TrianglePaint::Blocker));
    assert!(vol.triangle_color.iter().any(|s| s == "1"));
}

#[test]
fn parameter_modifier_config_load_and_roundtrip() {
    let xml = format!(
        r#"<?xml version="1.0"?>
<model unit="millimeter" xmlns="{CORE_NS}">
  <resources>
{}
{}
<object id="3" name="modded" type="model">
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
        cube_mesh_xml(2, "dense", 10.0, Vec3::new(5.0, 5.0, 5.0)),
    );
    let settings = r#"<?xml version="1.0" encoding="UTF-8"?>
<config>
  <object id="3">
<part id="1" subtype="normal_part">
  <metadata key="name" value="body"/>
</part>
<part id="2" subtype="modifier_part">
  <metadata key="name" value="dense"/>
  <metadata key="sparse_infill_density" value="100%"/>
  <metadata key="wall_loops" value="6"/>
</part>
  </object>
</config>"#;
    let model = load_3mf_bytes(&pack_files(&[
        (MODEL_PATH, &xml),
        (MODEL_SETTINGS_PATH, settings),
    ]))
    .unwrap();
    assert_eq!(model.objects[0].volumes.len(), 2);
    let vol = &model.objects[0].volumes[1];
    assert_eq!(vol.volume_type, bambu_model::VolumeType::Modifier);
    assert_eq!(
        vol.config.get("sparse_infill_density").map(String::as_str),
        Some("100%")
    );
    assert_eq!(vol.config.get("wall_loops").map(String::as_str), Some("6"));
    let bytes = write_model_3mf_bytes(&model).unwrap();
    {
        let mut zip = ZipArchive::new(Cursor::new(bytes.as_slice())).unwrap();
        let mut file = zip.by_name(MODEL_SETTINGS_PATH).unwrap();
        let mut body = String::new();
        file.read_to_string(&mut body).unwrap();
        assert!(body.contains("subtype=\"modifier_part\""), "{body}");
        assert!(body.contains("sparse_infill_density"), "{body}");
        assert!(body.contains("wall_loops"), "{body}");
    }
    let loaded = load_3mf_bytes(&bytes).unwrap();
    assert_eq!(
        loaded.objects[0].volumes[1].volume_type,
        bambu_model::VolumeType::Modifier
    );
    assert_eq!(
        loaded.objects[0].volumes[1]
            .config
            .get("sparse_infill_density")
            .map(String::as_str),
        Some("100%")
    );
}
