#![forbid(unsafe_code)]

mod bbs;
mod threemf;

use std::fs::File;
use std::io::{BufReader, BufWriter};
use std::path::Path;

use bambu_geom::TriangleMesh;
use bambu_model::Model;
use glam::Vec3;
use stl_io::{IndexedMesh, Normal, Triangle, Vertex};
use thiserror::Error;

pub use threemf::{
    load_3mf, load_3mf_bytes, write_3mf, write_3mf_bytes, write_model_3mf, write_model_3mf_bytes,
};

#[derive(Debug, Error)]
pub enum IoError {
    #[error("io: {0}")]
    Io(#[from] std::io::Error),
    #[error("zip: {0}")]
    Zip(#[from] zip::result::ZipError),
    #[error("xml: {0}")]
    Xml(#[from] quick_xml::Error),
    #[error("{0}")]
    Message(String),
}

pub fn load_stl(path: impl AsRef<Path>) -> Result<TriangleMesh, IoError> {
    let mut reader = BufReader::new(File::open(path)?);
    let indexed: IndexedMesh = stl_io::read_stl(&mut reader)?;
    let vertices = indexed
        .vertices
        .iter()
        .map(|v| Vec3::new(v[0], v[1], v[2]))
        .collect();
    let indices = indexed
        .faces
        .iter()
        .map(|f| {
            [
                f.vertices[0] as u32,
                f.vertices[1] as u32,
                f.vertices[2] as u32,
            ]
        })
        .collect();
    Ok(TriangleMesh { vertices, indices })
}

pub fn load_model_stl(path: impl AsRef<Path>) -> Result<Model, IoError> {
    let path = path.as_ref();
    let mesh = load_stl(path)?;
    let name = path
        .file_stem()
        .and_then(|s| s.to_str())
        .unwrap_or("object")
        .to_string();
    Ok(Model::from_mesh(name, mesh))
}

/// Load STL or 3MF by file extension.
///
/// Multi-plate Bambu 3MF uses the first plate (`Metadata/model_settings.config`).
pub fn load_mesh(path: impl AsRef<Path>) -> Result<TriangleMesh, IoError> {
    load_model(path)?
        .mesh_for_plate(0)
        .ok_or_else(|| IoError::Message("file contains no triangles".into()))
}

pub fn load_model(path: impl AsRef<Path>) -> Result<Model, IoError> {
    let path = path.as_ref();
    let ext = path
        .extension()
        .and_then(|e| e.to_str())
        .unwrap_or("")
        .to_ascii_lowercase();
    match ext.as_str() {
        "3mf" => load_3mf(path),
        "stl" | "" => load_model_stl(path),
        other => Err(IoError::Message(format!(
            "unsupported mesh format '.{other}' (stl|3mf)"
        ))),
    }
}

pub fn write_stl(path: impl AsRef<Path>, mesh: &TriangleMesh) -> Result<(), IoError> {
    let mut triangles = Vec::with_capacity(mesh.indices.len());
    for idx in &mesh.indices {
        let a = mesh.vertices[idx[0] as usize];
        let b = mesh.vertices[idx[1] as usize];
        let c = mesh.vertices[idx[2] as usize];
        let n = (b - a).cross(c - a).normalize_or_zero();
        triangles.push(Triangle {
            normal: Normal::new([n.x, n.y, n.z]),
            vertices: [
                Vertex::new([a.x, a.y, a.z]),
                Vertex::new([b.x, b.y, b.z]),
                Vertex::new([c.x, c.y, c.z]),
            ],
        });
    }
    let mut writer = BufWriter::new(File::create(path)?);
    stl_io::write_stl(&mut writer, triangles.iter())?;
    Ok(())
}
