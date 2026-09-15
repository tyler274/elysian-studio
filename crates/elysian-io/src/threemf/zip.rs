//! 3MF zip package layout (`[Content_Types]`, `_rels`, `3D/3dmodel.model`).

use std::io::{BufReader, Cursor, Read, Seek, Write};
use std::path::Path;
use std::time::Instant;

use rayon::prelude::*;
use zip::write::SimpleFileOptions;
use zip::{CompressionMethod, ZipArchive, ZipWriter};

use super::parse::{model_from_parsed, parse_xml_bytes, parse_xml_reader, ParsedModel};
use super::xml::normalize_model_path;
use crate::IoError;
use elysian_model::Model;

pub(crate) const MODEL_PATH: &str = "3D/3dmodel.model";
pub(crate) const MODEL_SETTINGS_PATH: &str = "Metadata/model_settings.config";
pub(crate) const PROJECT_SETTINGS_PATH: &str = "Metadata/project_settings.config";
const RELS_PATH: &str = "_rels/.rels";
const THUMBNAIL_CANDIDATES: &[&str] = &[
    "Metadata/plate_1.png",
    "Metadata/bbl_thumbnail.png",
    "Auxiliaries/.thumbnails/thumbnail_middle.png",
    "Auxiliaries/.thumbnails/thumbnail_small.png",
    "Auxiliaries/.thumbnails/thumbnail_3mf.png",
];

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

pub struct LoadTimings {
    pub open_ms: u128,
    pub inflate_ms: u128,
    pub parse_ms: u128,
    pub flatten_ms: u128,
    pub settings_ms: u128,
    pub triangles: usize,
    pub extra_models: usize,
}

/// Named 3MF open stages for a progress bar (parse is the long Belle path).
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum LoadStage {
    OpenArchive,
    ParseRoot,
    ParseObjects { done: usize, total: usize },
    Flatten,
    Settings,
}

impl LoadStage {
    pub fn label(&self) -> String {
        match self {
            Self::OpenArchive => "Opening archive…".into(),
            Self::ParseRoot => "Parsing 3D model…".into(),
            Self::ParseObjects { done, total } => {
                if *total == 0 {
                    "Parsing object meshes…".into()
                } else {
                    format!("Parsing object meshes ({done}/{total})…")
                }
            }
            Self::Flatten => "Flattening meshes…".into(),
            Self::Settings => "Applying project settings…".into(),
        }
    }

    pub fn fraction(&self) -> f32 {
        match self {
            Self::OpenArchive => 0.04,
            Self::ParseRoot => 0.10,
            Self::ParseObjects { done, total } => {
                0.12 + 0.40 * ratio(*done, *total)
            }
            Self::Flatten => 0.54,
            Self::Settings => 0.58,
        }
    }
}

fn ratio(done: usize, total: usize) -> f32 {
    if total == 0 {
        1.0
    } else {
        (done as f32 / total as f32).clamp(0.0, 1.0)
    }
}

impl LoadTimings {
    pub fn total_ms(&self) -> u128 {
        self.open_ms + self.inflate_ms + self.parse_ms + self.flatten_ms + self.settings_ms
    }
}

pub(super) fn load_package_seek_progress<R, F>(
    reader: R,
    open_ms: u128,
    mut progress: F,
) -> Result<(Model, LoadTimings), IoError>
where
    R: Read + Seek,
    F: FnMut(LoadStage),
{
    let t_open = Instant::now();
    let mut zip = ZipArchive::new(reader)?;
    let open_ms = open_ms + t_open.elapsed().as_millis();
    let entries = zip_entries(&zip);
    let original = root_model_name(&entries)?;
    let extra_names: Vec<(String, String)> = entries
        .iter()
        .filter(|(orig, norm)| {
            orig != &original && {
                let lower = norm.to_ascii_lowercase();
                lower.ends_with(".model") && !lower.contains("_rels")
            }
        })
        .cloned()
        .collect();

    progress(LoadStage::ParseRoot);
    let t_root = Instant::now();
    let root = {
        let file = zip.by_name(&original)?;
        parse_xml_reader(BufReader::new(file))?
    };
    let mut parse_ms = t_root.elapsed().as_millis();
    let mut inflate_ms = 0u128;

    let extra_total = extra_names.len();
    let extras = if extra_names.len() >= 2 {
        progress(LoadStage::ParseObjects {
            done: 0,
            total: extra_total,
        });
        let t_inf = Instant::now();
        let blobs: Result<Vec<(String, Vec<u8>)>, IoError> = extra_names
            .iter()
            .map(|(orig, norm)| Ok((norm.clone(), zip_entry_bytes(&mut zip, orig)?)))
            .collect();
        let blobs = blobs?;
        inflate_ms = t_inf.elapsed().as_millis();
        let t_parse = Instant::now();
        let parsed: Result<Vec<(String, ParsedModel)>, IoError> = blobs
            .par_iter()
            .map(|(path, bytes)| Ok((path.clone(), parse_xml_bytes(bytes)?)))
            .collect();
        parse_ms += t_parse.elapsed().as_millis();
        progress(LoadStage::ParseObjects {
            done: extra_total,
            total: extra_total,
        });
        parsed?
    } else {
        let mut out = Vec::new();
        for (i, (orig, norm)) in extra_names.iter().enumerate() {
            progress(LoadStage::ParseObjects {
                done: i,
                total: extra_total,
            });
            let t = Instant::now();
            let file = zip.by_name(orig)?;
            let parsed = parse_xml_reader(BufReader::new(file))?;
            parse_ms += t.elapsed().as_millis();
            out.push((norm.clone(), parsed));
        }
        if extra_total > 0 {
            progress(LoadStage::ParseObjects {
                done: extra_total,
                total: extra_total,
            });
        }
        out
    };

    let settings_xml = zip_optional(&mut zip, &entries, MODEL_SETTINGS_PATH)?;
    let project_json = zip_optional(&mut zip, &entries, PROJECT_SETTINGS_PATH)?;
    let extra_n = extras.len();

    progress(LoadStage::Flatten);
    let t_flat = Instant::now();
    let mut model = model_from_parsed(root, extras)?;
    let flatten_ms = t_flat.elapsed().as_millis();

    progress(LoadStage::Settings);
    let t_set = Instant::now();
    if let Some(settings_xml) = settings_xml {
        crate::bbs::apply(&mut model, &settings_xml)?;
    }
    if let Some(project_json) = project_json {
        model.settings = Some(
            elysian_config::settings_from_json(&project_json)
                .map_err(|err| IoError::Message(err.to_string()))?,
        );
    }
    let settings_ms = t_set.elapsed().as_millis();
    let triangles = model
        .objects
        .iter()
        .map(|o| {
            o.volumes
                .iter()
                .map(|v| v.mesh.indices.len())
                .sum::<usize>()
        })
        .sum();
    Ok((
        model,
        LoadTimings {
            open_ms,
            inflate_ms,
            parse_ms,
            flatten_ms,
            settings_ms,
            triangles,
            extra_models: extra_n,
        },
    ))
}

/// PNG bytes for a Home/recents preview. Geometry is not inflated.
pub fn read_3mf_thumbnail(path: impl AsRef<Path>) -> Result<Option<Vec<u8>>, IoError> {
    let file = std::fs::File::open(path.as_ref())?;
    read_3mf_thumbnail_seek(file)
}

pub fn read_3mf_thumbnail_bytes(bytes: &[u8]) -> Result<Option<Vec<u8>>, IoError> {
    read_3mf_thumbnail_seek(Cursor::new(bytes))
}

fn read_3mf_thumbnail_seek<R: Read + Seek>(reader: R) -> Result<Option<Vec<u8>>, IoError> {
    let mut zip = ZipArchive::new(reader)?;
    let entries = zip_entries(&zip);
    if let Some(rels) = zip_optional(&mut zip, &entries, RELS_PATH)? {
        if let Some(target) = thumbnail_from_rels(&rels) {
            let want = normalize_model_path(&target);
            if let Some(orig) = entries
                .iter()
                .find(|(_, n)| n.eq_ignore_ascii_case(&want))
                .map(|(o, _)| o.clone())
            {
                return Ok(Some(zip_entry_bytes(&mut zip, &orig)?));
            }
        }
    }
    for cand in THUMBNAIL_CANDIDATES {
        if let Some(orig) = entries
            .iter()
            .find(|(_, n)| n.eq_ignore_ascii_case(cand))
            .map(|(o, _)| o.clone())
        {
            return Ok(Some(zip_entry_bytes(&mut zip, &orig)?));
        }
    }
    Ok(None)
}

fn thumbnail_from_rels(rels: &str) -> Option<String> {
    for chunk in rels.split('<') {
        let lower = chunk.to_ascii_lowercase();
        if !lower.contains("relationship") {
            continue;
        }
        let png = lower.contains(".png")
            || lower.contains("thumbnail")
            || lower.contains("metadata/plate_");
        if !png {
            continue;
        }
        let target = attr_quoted(chunk, "Target").or_else(|| attr_quoted(chunk, "target"))?;
        if target.to_ascii_lowercase().ends_with(".png") {
            return Some(target);
        }
    }
    None
}

fn attr_quoted(src: &str, key: &str) -> Option<String> {
    let needle = format!("{key}=\"");
    let i = src.find(&needle)?;
    let rest = &src[i + needle.len()..];
    let end = rest.find('"')?;
    Some(rest[..end].to_string())
}

pub(super) fn write_package_with_thumbnail(
    model_xml: &str,
    settings_xml: &str,
    project_json: Option<&str>,
    thumbnail_png: Option<&[u8]>,
) -> Result<Vec<u8>, IoError> {
    let mut cursor = Cursor::new(Vec::new());
    {
        let mut zip = ZipWriter::new(&mut cursor);
        let opts = SimpleFileOptions::default().compression_method(CompressionMethod::Deflated);
        zip.start_file("[Content_Types].xml", opts)?;
        zip.write_all(CONTENT_TYPES.as_bytes())?;
        let rels = if thumbnail_png.is_some() {
            r#"<?xml version="1.0" encoding="UTF-8"?>
<Relationships xmlns="http://schemas.openxmlformats.org/package/2006/relationships">
  <Relationship Target="/3D/3dmodel.model" Id="rel-1" Type="http://schemas.microsoft.com/3dmanufacturing/2013/01/3dmodel"/>
  <Relationship Target="/Metadata/plate_1.png" Id="rel-2" Type="http://schemas.openxmlformats.org/package/2006/relationships/metadata/thumbnail"/>
</Relationships>
"#
        } else {
            RELS
        };
        zip.start_file(RELS_PATH, opts)?;
        zip.write_all(rels.as_bytes())?;
        zip.start_file(MODEL_PATH, opts)?;
        zip.write_all(model_xml.as_bytes())?;
        zip.start_file(MODEL_SETTINGS_PATH, opts)?;
        zip.write_all(settings_xml.as_bytes())?;
        if let Some(json) = project_json {
            zip.start_file(PROJECT_SETTINGS_PATH, opts)?;
            zip.write_all(json.as_bytes())?;
        }
        if let Some(png) = thumbnail_png {
            zip.start_file("Metadata/plate_1.png", opts)?;
            zip.write_all(png)?;
        }
        zip.finish()?;
    }
    Ok(cursor.into_inner())
}

fn zip_entries<R: Read + Seek>(zip: &ZipArchive<R>) -> Vec<(String, String)> {
    zip.file_names()
        .map(|n| (n.to_string(), normalize_zip_name(n)))
        .collect()
}

fn root_model_name(entries: &[(String, String)]) -> Result<String, IoError> {
    entries
        .iter()
        .find(|(_, n)| n.eq_ignore_ascii_case(MODEL_PATH))
        .or_else(|| {
            entries.iter().find(|(_, n)| {
                let lower = n.to_ascii_lowercase();
                lower.ends_with(".model") && !lower.contains("_rels")
            })
        })
        .map(|(orig, _)| orig.clone())
        .ok_or_else(|| IoError::Message("3MF has no 3D model part".into()))
}

fn zip_optional<R: Read + Seek>(
    zip: &mut ZipArchive<R>,
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

fn zip_entry_text<R: Read + Seek>(zip: &mut ZipArchive<R>, name: &str) -> Result<String, IoError> {
    let bytes = zip_entry_bytes(zip, name)?;
    String::from_utf8(bytes).map_err(|err| IoError::Message(format!("3MF utf-8: {err}")))
}

fn zip_entry_bytes<R: Read + Seek>(
    zip: &mut ZipArchive<R>,
    name: &str,
) -> Result<Vec<u8>, IoError> {
    let mut file = zip.by_name(name)?;
    let mut buf = Vec::with_capacity(file.size() as usize);
    file.read_to_end(&mut buf)?;
    Ok(buf)
}

fn normalize_zip_name(name: &str) -> String {
    name.replace('\\', "/")
}
