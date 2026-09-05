//! 3MF zip package layout (`[Content_Types]`, `_rels`, `3D/3dmodel.model`).

use std::io::{Cursor, Read, Write};

use zip::write::SimpleFileOptions;
use zip::{CompressionMethod, ZipArchive, ZipWriter};

use crate::IoError;

pub(crate) const MODEL_PATH: &str = "3D/3dmodel.model";
pub(crate) const MODEL_SETTINGS_PATH: &str = "Metadata/model_settings.config";
pub(crate) const PROJECT_SETTINGS_PATH: &str = "Metadata/project_settings.config";

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

pub(super) struct PackageTexts {
    pub model_xml: String,
    /// Other `*.model` parts (`3D/Objects/object_N.model`) keyed by zip path.
    pub extra_models: Vec<(String, String)>,
    pub settings_xml: Option<String>,
    pub project_json: Option<String>,
}

pub(super) fn read_package(bytes: &[u8]) -> Result<PackageTexts, IoError> {
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
    let model_xml = zip_entry_text(&mut zip, &original)?;
    let mut extra_models = Vec::new();
    for (orig, norm) in &entries {
        if orig == &original {
            continue;
        }
        let lower = norm.to_ascii_lowercase();
        if lower.ends_with(".model") && !lower.contains("_rels") {
            extra_models.push((norm.clone(), zip_entry_text(&mut zip, orig)?));
        }
    }
    let settings_xml = zip_optional(&mut zip, &entries, MODEL_SETTINGS_PATH)?;
    let project_json = zip_optional(&mut zip, &entries, PROJECT_SETTINGS_PATH)?;
    Ok(PackageTexts {
        model_xml,
        extra_models,
        settings_xml,
        project_json,
    })
}

pub(super) fn write_package(
    model_xml: &str,
    settings_xml: &str,
    project_json: Option<&str>,
) -> Result<Vec<u8>, IoError> {
    let mut cursor = Cursor::new(Vec::new());
    {
        let mut zip = ZipWriter::new(&mut cursor);
        let opts = SimpleFileOptions::default().compression_method(CompressionMethod::Deflated);
        zip.start_file("[Content_Types].xml", opts)?;
        zip.write_all(CONTENT_TYPES.as_bytes())?;
        zip.start_file("_rels/.rels", opts)?;
        zip.write_all(RELS.as_bytes())?;
        zip.start_file(MODEL_PATH, opts)?;
        zip.write_all(model_xml.as_bytes())?;
        zip.start_file(MODEL_SETTINGS_PATH, opts)?;
        zip.write_all(settings_xml.as_bytes())?;
        if let Some(json) = project_json {
            zip.start_file(PROJECT_SETTINGS_PATH, opts)?;
            zip.write_all(json.as_bytes())?;
        }
        zip.finish()?;
    }
    Ok(cursor.into_inner())
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

fn normalize_zip_name(name: &str) -> String {
    name.replace('\\', "/")
}
