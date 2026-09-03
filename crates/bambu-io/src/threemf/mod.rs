//! Core 3MF mesh import (`3D/3dmodel.model`) plus Bambu plates and parts.
//!
//! Geometry, units, build-item transforms, and component assemblies. When
//! `Metadata/model_settings.config` is present, object names, plates, part
//! subtype, and volume matrices are applied. `Metadata/project_settings.config`
//! carries process settings. Writers emit both files so plates, parts, and
//! settings round-trip. Triangle `paint_supports` / `paint_seam` /
//! `paint_fuzzy_skin` are applied; `paint_color` is stored. AMS stays ignored.

mod flatten;
mod parse;
mod write;
mod xml;
mod zip;

#[cfg(test)]
mod tests;

use std::path::Path;

use bambu_geom::TriangleMesh;
use bambu_model::Model;

use crate::IoError;

use self::parse::model_from_xml;
use self::write::model_xml_from_model;
use self::zip::{read_package, write_package};

#[cfg(test)]
pub(crate) use xml::CORE_NS;
#[cfg(test)]
pub(crate) use zip::{MODEL_PATH, MODEL_SETTINGS_PATH, PROJECT_SETTINGS_PATH};

pub fn load_3mf(path: impl AsRef<Path>) -> Result<Model, IoError> {
    let bytes = std::fs::read(path.as_ref())?;
    load_3mf_bytes(&bytes)
}

pub fn load_3mf_bytes(bytes: &[u8]) -> Result<Model, IoError> {
    let pack = read_package(bytes)?;
    let mut model = model_from_xml(&pack.model_xml)?;
    if let Some(settings_xml) = pack.settings_xml {
        crate::bbs::apply(&mut model, &settings_xml)?;
    }
    if let Some(project_json) = pack.project_json {
        model.settings = Some(
            bambu_config::settings_from_json(&project_json)
                .map_err(|err| IoError::Message(err.to_string()))?,
        );
    }
    Ok(model)
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
    let project = if let Some(slice) = &model.settings {
        Some(
            bambu_config::project_settings_json(slice)
                .map_err(|err| IoError::Message(err.to_string()))?,
        )
    } else {
        None
    };
    write_package(&exported.xml, &settings, project.as_deref())
}

pub fn write_model_3mf(path: impl AsRef<Path>, model: &Model) -> Result<(), IoError> {
    std::fs::write(path, write_model_3mf_bytes(model)?)?;
    Ok(())
}
