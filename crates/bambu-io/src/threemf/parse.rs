//! Core 3MF `3dmodel.model` XML → [`ParsedModel`].

use std::collections::BTreeMap;

use bambu_model::{Model, TrianglePaint};
use glam::{Mat4, Vec3};
use quick_xml::events::Event;
use quick_xml::Reader;

use super::flatten::flatten_model;
use super::xml::{attr, attr_f32, attr_u32, parse_transform, unit_factor};
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
    pub components: Vec<(u32, Mat4)>,
}

#[derive(Default)]
pub(super) struct ParsedModel {
    pub unit_factor: f32,
    pub objects: BTreeMap<u32, ObjectRec>,
    pub build: Vec<(u32, Mat4)>,
    pub current_id: Option<u32>,
}

pub(super) fn model_from_xml(xml: &str) -> Result<Model, IoError> {
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
                        let rec = parsed.objects.get_mut(&id).ok_or_else(|| {
                            IoError::Message("3MF triangle object missing".into())
                        })?;
                        rec.triangles.push(tri);
                        rec.triangle_support.push(TrianglePaint::from_hex(
                            attr(&e, b"paint_supports").as_deref().unwrap_or(""),
                        ));
                        rec.triangle_seam.push(TrianglePaint::from_hex(
                            attr(&e, b"paint_seam").as_deref().unwrap_or(""),
                        ));
                        rec.triangle_fuzzy_skin.push(TrianglePaint::from_hex(
                            attr(&e, b"paint_fuzzy_skin").as_deref().unwrap_or(""),
                        ));
                        rec.triangle_color
                            .push(attr(&e, b"paint_color").unwrap_or_default());
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
