//! Bambu `Metadata/model_settings.config` (plates, object names).
//!
//! Paint, AMS mapping, volume matrices, and assemble-view transforms are
//! ignored until a later bbs_3mf pass.

use std::collections::BTreeMap;

use bambu_model::{Model, PartPlate};
use quick_xml::events::Event;
use quick_xml::Reader;

use crate::IoError;

#[derive(Clone, Copy)]
enum Ctx {
    Root,
    Object(u32),
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

/// Apply Bambu model_settings onto a flattened core-3MF [`Model`].
pub fn apply(model: &mut Model, xml: &str) -> Result<(), IoError> {
    let (names, plates) = parse(xml)?;
    if names.is_empty() && plates.is_empty() {
        return Ok(());
    }
    for obj in &mut model.objects {
        if let Some(name) = names.get(&obj.object_id) {
            obj.name = name.clone();
        }
    }
    if plates.is_empty() {
        return Ok(());
    }
    let mut out = Vec::with_capacity(plates.len());
    for plate in plates {
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

/// Serialize plates and object names as Bambu `model_settings.config`.
///
/// `object_ids[i]` is the 3MF resource id for `model.objects[i]` (`0` skips).
pub fn write(model: &Model, object_ids: &[u32]) -> String {
    let mut out = String::from("<?xml version=\"1.0\" encoding=\"UTF-8\"?>\n<config>\n");
    for (i, obj) in model.objects.iter().enumerate() {
        let Some(&id) = object_ids.get(i) else {
            continue;
        };
        if id == 0 {
            continue;
        }
        out.push_str(&format!(
            "  <object id=\"{id}\">\n    <metadata key=\"name\" value=\"{}\"/>\n  </object>\n",
            xml_escape(&obj.name)
        ));
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

fn parse(xml: &str) -> Result<(BTreeMap<u32, String>, Vec<PlateRec>), IoError> {
    let mut reader = Reader::from_str(xml);
    reader.config_mut().trim_text(true);
    let mut buf = Vec::new();
    let mut ctx = Ctx::Root;
    let mut names = BTreeMap::new();
    let mut plates = Vec::new();
    let mut plate = PlateRec::default();
    let mut inst = InstanceRec::default();

    loop {
        match reader.read_event_into(&mut buf)? {
            Event::Start(e) => {
                let local = e.local_name();
                match local.as_ref() {
                    b"object" => ctx = Ctx::Object(attr_u32(&e, b"id")),
                    b"plate" => {
                        plate = PlateRec::default();
                        ctx = Ctx::Plate;
                    }
                    b"model_instance" => {
                        inst = InstanceRec::default();
                        ctx = Ctx::Instance;
                    }
                    b"part" | b"volume" | b"assemble" | b"assemble_item" | b"filament"
                    | b"mixed_filament" | b"nozzle" | b"ams" => {
                        ctx = Ctx::Skip;
                    }
                    b"metadata" => apply_metadata(ctx, &e, &mut names, &mut plate, &mut inst),
                    _ => {}
                }
            }
            Event::Empty(e) => {
                let local = e.local_name();
                if local.as_ref() == b"metadata" {
                    apply_metadata(ctx, &e, &mut names, &mut plate, &mut inst);
                }
            }
            Event::End(e) => {
                let local = e.local_name();
                match local.as_ref() {
                    b"object" | b"part" | b"volume" | b"assemble" | b"assemble_item"
                    | b"filament" | b"mixed_filament" | b"nozzle" | b"ams" => {
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
    Ok((names, plates))
}

fn apply_metadata(
    ctx: Ctx,
    e: &quick_xml::events::BytesStart<'_>,
    names: &mut BTreeMap<u32, String>,
    plate: &mut PlateRec,
    inst: &mut InstanceRec,
) {
    match ctx {
        Ctx::Object(id) => {
            if attr(e, b"key").as_deref() == Some("name") {
                if let Some(v) = attr(e, b"value") {
                    names.insert(id, v);
                }
            }
        }
        Ctx::Plate => apply_plate_meta(plate, e),
        Ctx::Instance => apply_instance_meta(inst, e),
        Ctx::Root | Ctx::Skip => {}
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
