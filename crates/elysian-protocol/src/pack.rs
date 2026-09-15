//! Minimal `.gcode.3mf` wrapper so LAN `project_file` can point at `Metadata/plate_1.gcode`.

use std::io::{Cursor, Write};

use thiserror::Error;
use zip::write::SimpleFileOptions;
use zip::{CompressionMethod, ZipWriter};

#[derive(Debug, Error)]
pub enum PackError {
    #[error("zip: {0}")]
    Zip(#[from] zip::result::ZipError),
    #[error("io: {0}")]
    Io(#[from] std::io::Error),
}

const CONTENT_TYPES: &str = r#"<?xml version="1.0" encoding="UTF-8"?>
<Types xmlns="http://schemas.openxmlformats.org/package/2006/content-types">
  <Default Extension="rels" ContentType="application/vnd.openxmlformats-package.relationships+xml"/>
  <Default Extension="gcode" ContentType="text/plain"/>
  <Default Extension="xml" ContentType="application/xml"/>
</Types>
"#;

const RELS: &str = r#"<?xml version="1.0" encoding="UTF-8"?>
<Relationships xmlns="http://schemas.openxmlformats.org/package/2006/relationships">
  <Relationship Target="/Metadata/plate_1.gcode" Id="rel-1" Type="http://schemas.microsoft.com/3dmanufacturing/2013/01/3dmodel"/>
</Relationships>
"#;

/// Pack G-code into a zip the printer accepts as a LAN project file.
pub fn pack_gcode_3mf(gcode: &str) -> Result<Vec<u8>, PackError> {
    let mut cursor = Cursor::new(Vec::new());
    {
        let mut zip = ZipWriter::new(&mut cursor);
        let opts = SimpleFileOptions::default().compression_method(CompressionMethod::Deflated);
        zip.start_file("[Content_Types].xml", opts)?;
        zip.write_all(CONTENT_TYPES.as_bytes())?;
        zip.start_file("_rels/.rels", opts)?;
        zip.write_all(RELS.as_bytes())?;
        zip.start_file("Metadata/plate_1.gcode", opts)?;
        zip.write_all(gcode.as_bytes())?;
        zip.finish()?;
    }
    Ok(cursor.into_inner())
}

/// Keep FTPS / MQTT names in `[A-Za-z0-9._-]`.
pub fn sanitize_remote_name(name: &str) -> String {
    let stem = name
        .rsplit(['/', '\\'])
        .next()
        .unwrap_or(name)
        .trim()
        .trim_end_matches(".gcode.3mf")
        .trim_end_matches(".gcode")
        .trim_end_matches(".3mf");
    let mut out = String::new();
    for ch in stem.chars() {
        if ch.is_ascii_alphanumeric() || matches!(ch, '.' | '_' | '-') {
            out.push(ch);
        } else if !out.ends_with('-') {
            out.push('-');
        }
    }
    let out = out.trim_matches(|c| c == '-' || c == '.').to_string();
    if out.is_empty() {
        "job".into()
    } else {
        out
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::Read;
    use zip::ZipArchive;

    #[test]
    fn packs_plate_gcode() {
        let bytes = pack_gcode_3mf("; LAYER\nG1 X1\n").unwrap();
        let mut zip = ZipArchive::new(Cursor::new(bytes)).unwrap();
        let mut file = zip.by_name("Metadata/plate_1.gcode").unwrap();
        let mut body = String::new();
        file.read_to_string(&mut body).unwrap();
        assert!(body.contains("G1 X1"));
    }

    #[test]
    fn sanitizes_names() {
        assert_eq!(sanitize_remote_name("cube.gcode"), "cube");
        assert_eq!(sanitize_remote_name("/tmp/My Cube (1).gcode"), "My-Cube-1");
        assert_eq!(sanitize_remote_name("..."), "job");
    }
}
