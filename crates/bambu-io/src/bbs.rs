//! Bambu `Metadata/model_settings.config` (plates, object names, parts).
//!
//! Paint, AMS mapping, and assemble-view transforms stay ignored.

use std::collections::BTreeMap;

use bambu_model::{Model, ModelVolume, PartPlate, VolumeType};
use glam::Mat4;
use quick_xml::events::Event;
use quick_xml::Reader;

use crate::IoError;

#[derive(Clone, Copy)]
enum Ctx {
    Root,
    Object,
    Part,
    Plate,
    Instance,
    Skip,
}

#[derive(Default)]
struct PlateRec {
    index: i32,
    name: String,
    locked: bool,
    instances: Vec<(u32, u32)>,
}

#[derive(Default)]
struct InstanceRec {
    object_id: u32,
    instance_id: u32,
}

struct PartRec {
    id: u32,
    subtype: VolumeType,
    name: String,
    matrix: Mat4,
}

impl Default for PartRec {
    fn default() -> Self {
        Self {
            id: 0,
            subtype: VolumeType::ModelPart,
            name: String::new(),
            matrix: Mat4::IDENTITY,
        }
    }
}

/// Apply Bambu model_settings onto a flattened core-3MF [`Model`].
pub fn apply(model: &mut Model, xml: &str) -> Result<(), IoError> {
    let parsed = parse(xml)?;
    if parsed.names.is_empty() && parsed.plates.is_empty() && parsed.parts.is_empty() {
        return Ok(());
    }
    for obj in &mut model.objects {
        if let Some(name) = parsed.names.get(&obj.object_id) {
            obj.name = name.clone();
        }
        if let Some(parts) = parsed.parts.get(&obj.object_id) {
            apply_parts(obj, parts);
        }
    }
    if parsed.plates.is_empty() {
        return Ok(());
    }
    let mut out = Vec::with_capacity(parsed.plates.len());
    for plate in parsed.plates {
        let mut indices = Vec::new();
        for &(oid, iid) in &plate.instances {
            for (i, obj) in model.objects.iter().enumerate() {
                if obj.object_id == oid && obj.instance_id == iid {
                    indices.push(i);
                }
            }
        }
        let name = if plate.name.is_empty() {
            format!("Plate {}", plate.index.max(1))
        } else {
            plate.name
        };
        out.push(PartPlate {
            name,
            object_indices: indices,
            locked: plate.locked,
        });
    }
    model.plates = out;
    Ok(())
}

fn apply_parts(obj: &mut bambu_model::ModelObject, parts: &[PartRec]) {
    for vol in &mut obj.volumes {
        let Some(part) = find_part(parts, vol.part_id) else {
            continue;
        };
        apply_part_to_volume(vol, part);
    }
    if obj.volumes.is_empty() {
        if let Some(part) = parts.first() {
            let mut vol = ModelVolume::model_part(obj.name.clone(), obj.mesh.clone(), part.id);
            apply_part_to_volume(&mut vol, part);
            obj.volumes.push(vol);
        }
    }
    obj.rebuild_printable_mesh();
}

fn find_part(parts: &[PartRec], part_id: u32) -> Option<&PartRec> {
    parts
        .iter()
        .find(|p| p.id == part_id)
        .or_else(|| (parts.len() == 1).then_some(&parts[0]))
}

fn apply_part_to_volume(vol: &mut ModelVolume, part: &PartRec) {
    vol.volume_type = part.subtype;
    vol.part_id = part.id;
    if !part.name.is_empty() {
        vol.name = part.name.clone();
    }
    if part.matrix != Mat4::IDENTITY {
        vol.mesh.transform(part.matrix);
        vol.matrix = part.matrix;
    }
}

/// Serialize plates, object names, and part subtype/matrix as Bambu `model_settings.config`.
///
/// `object_ids[i]` is the 3MF resource id for `model.objects[i]` (`0` skips).
/// `volume_ids[i][j]` is the 3MF id of that object's j-th volume.
pub fn write(model: &Model, object_ids: &[u32], volume_ids: &[Vec<u32>]) -> String {
    let mut out = String::from("<?xml version=\"1.0\" encoding=\"UTF-8\"?>\n<config>\n");
    for (i, obj) in model.objects.iter().enumerate() {
        let Some(&id) = object_ids.get(i) else {
            continue;
        };
        if id == 0 {
            continue;
        }
        out.push_str(&format!("  <object id=\"{id}\">\n"));
        out.push_str(&format!(
            "    <metadata key=\"name\" value=\"{}\"/>\n",
            xml_escape(&obj.name)
        ));
        let vols = obj.volumes_or_mesh();
        let ids = volume_ids.get(i).cloned().unwrap_or_default();
        for (j, vol) in vols.iter().enumerate() {
            let pid = ids.get(j).copied().filter(|n| *n != 0).unwrap_or(id);
            out.push_str(&format!(
                "    <part id=\"{pid}\" subtype=\"{}\">\n",
                vol.volume_type.as_str()
            ));
            out.push_str(&format!(
                "      <metadata key=\"name\" value=\"{}\"/>\n",
                xml_escape(&vol.name)
            ));
            out.push_str(&format!(
                "      <metadata key=\"matrix\" value=\"{}\"/>\n",
                matrix_string(vol.matrix)
            ));
            out.push_str("    </part>\n");
        }
        out.push_str("  </object>\n");
    }
    for (pi, plate) in model.plates.iter().enumerate() {
        let idx = pi + 1;
        out.push_str("  <plate>\n");
        out.push_str(&format!(
            "    <metadata key=\"plater_id\" value=\"{idx}\"/>\n"
        ));
        out.push_str(&format!(
            "    <metadata key=\"plater_name\" value=\"{}\"/>\n",
            xml_escape(&plate.name)
        ));
        let locked = if plate.locked { "true" } else { "false" };
        out.push_str(&format!(
            "    <metadata key=\"locked\" value=\"{locked}\"/>\n"
        ));
        for &oi in &plate.object_indices {
            let Some(&id) = object_ids.get(oi) else {
                continue;
            };
            if id == 0 {
                continue;
            }
            out.push_str(&format!(
                "    <model_instance>\n      <metadata key=\"object_id\" value=\"{id}\"/>\n      <metadata key=\"instance_id\" value=\"0\"/>\n    </model_instance>\n"
            ));
        }
        out.push_str("  </plate>\n");
    }
    out.push_str("</config>\n");
    out
}

fn xml_escape(s: &str) -> String {
    s.replace('&', "&amp;")
        .replace('<', "&lt;")
        .replace('>', "&gt;")
        .replace('"', "&quot;")
}

/// Eigen/Bambu row-major 4×4 (`transform3d_from_string`).
fn matrix_string(m: Mat4) -> String {
    let a = m.to_cols_array();
    format!(
        "{} {} {} {} {} {} {} {} {} {} {} {} {} {} {} {}",
        a[0],
        a[4],
        a[8],
        a[12],
        a[1],
        a[5],
        a[9],
        a[13],
        a[2],
        a[6],
        a[10],
        a[14],
        a[3],
        a[7],
        a[11],
        a[15]
    )
}

/// Parse the 16-number row-major matrix used in `metadata key="matrix"`.
pub fn parse_matrix(s: &str) -> Mat4 {
    let nums: Vec<f32> = s
        .split_whitespace()
        .filter_map(|t| t.parse().ok())
        .collect();
    if nums.len() != 16 {
        return Mat4::IDENTITY;
    }
    Mat4::from_cols(
        glam::Vec4::new(nums[0], nums[4], nums[8], nums[12]),
        glam::Vec4::new(nums[1], nums[5], nums[9], nums[13]),
        glam::Vec4::new(nums[2], nums[6], nums[10], nums[14]),
        glam::Vec4::new(nums[3], nums[7], nums[11], nums[15]),
    )
}

struct ParsedSettings {
    names: BTreeMap<u32, String>,
    parts: BTreeMap<u32, Vec<PartRec>>,
    plates: Vec<PlateRec>,
}

fn parse(xml: &str) -> Result<ParsedSettings, IoError> {
    let mut reader = Reader::from_str(xml);
    reader.config_mut().trim_text(true);
    let mut buf = Vec::new();
    let mut ctx = Ctx::Root;
    let mut names = BTreeMap::new();
    let mut parts: BTreeMap<u32, Vec<PartRec>> = BTreeMap::new();
    let mut plates = Vec::new();
    let mut plate = PlateRec::default();
    let mut inst = InstanceRec::default();
    let mut part = PartRec::default();
    let mut object_id = 0u32;

    loop {
        match reader.read_event_into(&mut buf)? {
            Event::Start(e) => {
                let local = e.local_name();
                match local.as_ref() {
                    b"object" => {
                        object_id = attr_u32(&e, b"id");
                        ctx = Ctx::Object;
                    }
                    b"part" | b"volume" => {
                        part = PartRec {
                            id: attr_u32(&e, b"id"),
                            subtype: VolumeType::from_subtype(
                                attr(&e, b"subtype").as_deref().unwrap_or(""),
                            ),
                            ..PartRec::default()
                        };
                        ctx = Ctx::Part;
                    }
                    b"plate" => {
                        plate = PlateRec::default();
                        ctx = Ctx::Plate;
                    }
                    b"model_instance" => {
                        inst = InstanceRec::default();
                        ctx = Ctx::Instance;
                    }
                    b"assemble" | b"assemble_item" | b"filament" | b"mixed_filament"
                    | b"nozzle" | b"ams" => {
                        ctx = Ctx::Skip;
                    }
                    b"metadata" => {
                        apply_metadata(
                            ctx, object_id, &e, &mut names, &mut plate, &mut inst, &mut part,
                        );
                    }
                    _ => {}
                }
            }
            Event::Empty(e) => {
                let local = e.local_name();
                if local.as_ref() == b"metadata" {
                    apply_metadata(
                        ctx, object_id, &e, &mut names, &mut plate, &mut inst, &mut part,
                    );
                }
            }
            Event::End(e) => {
                let local = e.local_name();
                match local.as_ref() {
                    b"object" => {
                        ctx = Ctx::Root;
                    }
                    b"part" | b"volume" => {
                        parts
                            .entry(object_id)
                            .or_default()
                            .push(std::mem::take(&mut part));
                        ctx = Ctx::Object;
                    }
                    b"assemble" | b"assemble_item" | b"filament" | b"mixed_filament"
                    | b"nozzle" | b"ams" => {
                        ctx = Ctx::Root;
                    }
                    b"plate" => {
                        if plate.index <= 0 {
                            plate.index = (plates.len() as i32) + 1;
                        }
                        plates.push(std::mem::take(&mut plate));
                        ctx = Ctx::Root;
                    }
                    b"model_instance" => {
                        plate.instances.push((inst.object_id, inst.instance_id));
                        ctx = Ctx::Plate;
                    }
                    _ => {}
                }
            }
            Event::Eof => break,
            _ => {}
        }
        buf.clear();
    }

    plates.sort_by_key(|p| p.index);
    Ok(ParsedSettings {
        names,
        parts,
        plates,
    })
}

fn apply_metadata(
    ctx: Ctx,
    object_id: u32,
    e: &quick_xml::events::BytesStart<'_>,
    names: &mut BTreeMap<u32, String>,
    plate: &mut PlateRec,
    inst: &mut InstanceRec,
    part: &mut PartRec,
) {
    match ctx {
        Ctx::Object => {
            if attr(e, b"key").as_deref() == Some("name") {
                if let Some(v) = attr(e, b"value") {
                    names.insert(object_id, v);
                }
            }
        }
        Ctx::Part => apply_part_meta(part, e),
        Ctx::Plate => apply_plate_meta(plate, e),
        Ctx::Instance => apply_instance_meta(inst, e),
        Ctx::Root | Ctx::Skip => {}
    }
}

fn apply_part_meta(part: &mut PartRec, e: &quick_xml::events::BytesStart<'_>) {
    let Some(key) = attr(e, b"key") else {
        return;
    };
    let Some(value) = attr(e, b"value") else {
        return;
    };
    match key.as_str() {
        "name" => part.name = value,
        "matrix" => part.matrix = parse_matrix(&value),
        "volume_type" | "part_type" => part.subtype = VolumeType::from_subtype(&value),
        _ => {}
    }
}

fn apply_plate_meta(plate: &mut PlateRec, e: &quick_xml::events::BytesStart<'_>) {
    let Some(key) = attr(e, b"key") else {
        return;
    };
    let Some(value) = attr(e, b"value") else {
        return;
    };
    match key.as_str() {
        "plater_id" => {
            if let Ok(n) = value.parse() {
                plate.index = n;
            }
        }
        "plater_name" => plate.name = value,
        "locked" => {
            plate.locked = value.eq_ignore_ascii_case("true") || value == "1";
        }
        _ => {}
    }
}

fn apply_instance_meta(inst: &mut InstanceRec, e: &quick_xml::events::BytesStart<'_>) {
    let Some(key) = attr(e, b"key") else {
        return;
    };
    let Some(value) = attr(e, b"value") else {
        return;
    };
    match key.as_str() {
        "object_id" => {
            if let Ok(n) = value.parse() {
                inst.object_id = n;
            }
        }
        "instance_id" => {
            if let Ok(n) = value.parse() {
                inst.instance_id = n;
            }
        }
        _ => {}
    }
}

fn attr(e: &quick_xml::events::BytesStart<'_>, key: &[u8]) -> Option<String> {
    e.try_get_attribute(key)
        .ok()
        .flatten()
        .and_then(|a| String::from_utf8(a.value.into_owned()).ok())
}

fn attr_u32(e: &quick_xml::events::BytesStart<'_>, key: &[u8]) -> u32 {
    attr(e, key).and_then(|s| s.parse().ok()).unwrap_or(0)
}
