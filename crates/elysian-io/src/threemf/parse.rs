//! Core 3MF `3dmodel.model` XML → [`ParsedModel`].

use std::collections::BTreeMap;
use std::io::BufRead;

use elysian_model::{Model, TrianglePaint};
use glam::{Mat4, Vec3};
use quick_xml::events::Event;
use quick_xml::Reader;

use super::flatten::flatten_files;
use super::xml::{
    attr, attr_f32, attr_path, attr_raw, attr_u32, normalize_model_path, parse_transform,
    unit_factor,
};
use super::zip::MODEL_PATH;
use crate::IoError;

#[derive(Default)]
pub(super) struct ObjectRec {
    pub name: String,
    pub vertices: Vec<Vec3>,
    pub triangles: Vec<[u32; 3]>,
    pub triangle_support: Vec<TrianglePaint>,
    pub triangle_seam: Vec<TrianglePaint>,
    pub triangle_fuzzy_skin: Vec<TrianglePaint>,
    pub triangle_color: Vec<String>,
    /// `(p:path or empty for this file, objectid, transform)`.
    pub components: Vec<(String, u32, Mat4)>,
}

#[derive(Default)]
pub(super) struct ParsedModel {
    pub unit_factor: f32,
    pub objects: BTreeMap<u32, ObjectRec>,
    pub build: Vec<(u32, Mat4)>,
    pub current_id: Option<u32>,
}

pub(super) fn model_from_parsed(
    root: ParsedModel,
    extras: Vec<(String, ParsedModel)>,
) -> Result<Model, IoError> {
    let mut files = BTreeMap::new();
    files.insert(MODEL_PATH.to_string(), root);
    for (path, parsed) in extras {
        files.insert(normalize_model_path(&path), parsed);
    }
    flatten_files(&files, MODEL_PATH)
}

pub(super) fn parse_xml_bytes(bytes: &[u8]) -> Result<ParsedModel, IoError> {
    parse_reader(Reader::from_reader(std::io::Cursor::new(bytes)))
}

pub(super) fn parse_xml_reader<R: BufRead>(reader: R) -> Result<ParsedModel, IoError> {
    parse_reader(Reader::from_reader(reader))
}

fn parse_reader<R: BufRead>(mut reader: Reader<R>) -> Result<ParsedModel, IoError> {
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
                        let rec = parsed.objects.get_mut(&id).ok_or_else(|| {
                            IoError::Message("3MF triangle object missing".into())
                        })?;
                        rec.triangles.push(tri);
                        let i = rec.triangles.len() - 1;
                        set_paint(
                            &mut rec.triangle_support,
                            i,
                            attr_paint(&e, b"paint_supports"),
                        );
                        set_paint(&mut rec.triangle_seam, i, attr_paint(&e, b"paint_seam"));
                        set_paint(
                            &mut rec.triangle_fuzzy_skin,
                            i,
                            attr_paint(&e, b"paint_fuzzy_skin"),
                        );
                        set_color(&mut rec.triangle_color, i, attr_color(&e, b"paint_color"));
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
                        let path = attr_path(&e).unwrap_or_default();
                        parsed
                            .objects
                            .get_mut(&id)
                            .ok_or_else(|| IoError::Message("3MF component object missing".into()))?
                            .components
                            .push((path, child, xf));
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

    Ok(parsed)
}

fn attr_paint(e: &quick_xml::events::BytesStart<'_>, key: &[u8]) -> TrianglePaint {
    attr_raw(e, key)
        .and_then(|v| {
            std::str::from_utf8(v.as_ref())
                .ok()
                .map(TrianglePaint::from_hex)
        })
        .unwrap_or(TrianglePaint::None)
}

fn attr_color(e: &quick_xml::events::BytesStart<'_>, key: &[u8]) -> String {
    attr(e, key).unwrap_or_default()
}

fn set_paint(vec: &mut Vec<TrianglePaint>, index: usize, paint: TrianglePaint) {
    if paint == TrianglePaint::None && vec.is_empty() {
        return;
    }
    if vec.len() <= index {
        vec.resize(index + 1, TrianglePaint::None);
    }
    vec[index] = paint;
}

fn set_color(vec: &mut Vec<String>, index: usize, color: String) {
    if color.is_empty() && vec.is_empty() {
        return;
    }
    if vec.len() <= index {
        vec.resize(index + 1, String::new());
    }
    vec[index] = color;
}
