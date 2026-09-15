//! Core 3MF mesh import (`3D/3dmodel.model`) plus Bambu plates and parts.
//!
//! Geometry, units, build-item transforms, and component assemblies
//! (`p:path` into `3D/Objects/*.model`). When
//! `Metadata/model_settings.config` is present, object names, plates, part
//! subtype, and volume matrices are applied. `Metadata/project_settings.config`
//! carries process settings. Writers emit both files so plates, parts, and
//! settings round-trip. Triangle `paint_supports` / `paint_seam` /
//! `paint_fuzzy_skin` are applied; `paint_color` splits AMS regions at slice time.

mod flatten;
mod parse;
mod write;
mod xml;
mod zip;

#[cfg(test)]
mod tests;

use std::io::Cursor;
use std::path::Path;
use std::time::Instant;

use elysian_geom::TriangleMesh;
use elysian_model::Model;

use crate::IoError;

use self::write::model_xml_from_model;

#[cfg(test)]
pub(crate) use xml::CORE_NS;
#[cfg(test)]
pub(crate) use zip::{MODEL_PATH, MODEL_SETTINGS_PATH, PROJECT_SETTINGS_PATH};

pub use zip::{read_3mf_thumbnail, read_3mf_thumbnail_bytes, LoadStage, LoadTimings};

pub fn load_3mf(path: impl AsRef<Path>) -> Result<Model, IoError> {
    load_3mf_timed(path).map(|(model, _)| model)
}

/// Timed 3MF open used by `elysian-cli open-timing`. Opens the zip from a file
/// handle (no extra full-file copy).
pub fn load_3mf_timed(path: impl AsRef<Path>) -> Result<(Model, LoadTimings), IoError> {
    load_3mf_with_progress(path, |_| {})
}

/// Timed 3MF open with stage callbacks for a progress UI.
pub fn load_3mf_with_progress(
    path: impl AsRef<Path>,
    mut progress: impl FnMut(LoadStage),
) -> Result<(Model, LoadTimings), IoError> {
    progress(LoadStage::OpenArchive);
    let t = Instant::now();
    let file = std::fs::File::open(path.as_ref())?;
    let open_ms = t.elapsed().as_millis();
    zip::load_package_seek_progress(file, open_ms, progress)
}

pub fn load_3mf_bytes(bytes: &[u8]) -> Result<Model, IoError> {
    load_3mf_bytes_with_progress(bytes, |_| {})
}

pub fn load_3mf_bytes_with_progress(
    bytes: &[u8],
    mut progress: impl FnMut(LoadStage),
) -> Result<Model, IoError> {
    progress(LoadStage::OpenArchive);
    zip::load_package_seek_progress(Cursor::new(bytes), 0, progress).map(|(model, _)| model)
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
    write_model_3mf_bytes_with_thumbnail(model, None)
}

pub fn write_model_3mf_bytes_with_thumbnail(
    model: &Model,
    thumbnail_png: Option<&[u8]>,
) -> Result<Vec<u8>, IoError> {
    let exported = model_xml_from_model(model)?;
    let settings = crate::bbs::write(model, &exported.object_ids, &exported.volume_ids);
    let project = if let Some(slice) = &model.settings {
        Some(
            elysian_config::project_settings_json(slice)
                .map_err(|err| IoError::Message(err.to_string()))?,
        )
    } else {
        None
    };
    zip::write_package_with_thumbnail(&exported.xml, &settings, project.as_deref(), thumbnail_png)
}

pub fn write_model_3mf(path: impl AsRef<Path>, model: &Model) -> Result<(), IoError> {
    std::fs::write(path, write_model_3mf_bytes(model)?)?;
    Ok(())
}
