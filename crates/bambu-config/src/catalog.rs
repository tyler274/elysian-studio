//! SpoolmanDB product catalog (vendor JSON + materials.json).
//!
//! Expands like `SpoolmanDB/scripts/compile_filaments.py`. This is not a
//! TabFilament preset: bind applies physics/temps, then optional Generic BBL.

use std::path::{Path, PathBuf};

use serde::Deserialize;
use serde_json::Value;

use crate::bbl::normalize_filament_colour;
use crate::{ConfigError, SliceSettings};

#[derive(Debug, Clone, PartialEq)]
pub struct CatalogMaterial {
    pub material: String,
    pub density: f64,
    pub extruder_temp: Option<u16>,
    pub bed_temp: Option<u16>,
}

#[derive(Debug, Clone, PartialEq)]
pub struct CatalogFilament {
    pub external_id: String,
    pub manufacturer: String,
    pub name: String,
    pub material: String,
    pub density: f64,
    pub diameter: f64,
    pub net_weight: f64,
    pub spool_weight: f64,
    pub color_name: String,
    pub color_hex: String,
    pub extruder_temp: Option<u16>,
    pub bed_temp: Option<u16>,
}

#[derive(Debug, Clone, Default, PartialEq)]
pub struct CatalogIndex {
    pub materials: Vec<CatalogMaterial>,
    pub filaments: Vec<CatalogFilament>,
}

impl CatalogIndex {
    pub fn material(&self, name: &str) -> Option<&CatalogMaterial> {
        self.materials
            .iter()
            .find(|m| m.material.eq_ignore_ascii_case(name))
    }

    pub fn by_external_id(&self, id: &str) -> Option<&CatalogFilament> {
        self.filaments.iter().find(|f| f.external_id == id)
    }

    /// Case-insensitive substring match; capped so iced never sees the full SKU list.
    pub fn search(&self, query: &str, limit: usize) -> Vec<&CatalogFilament> {
        let q = query.trim().to_ascii_lowercase();
        self.filaments
            .iter()
            .filter(|f| {
                if q.is_empty() {
                    return true;
                }
                f.manufacturer.to_ascii_lowercase().contains(&q)
                    || f.material.to_ascii_lowercase().contains(&q)
                    || f.name.to_ascii_lowercase().contains(&q)
                    || f.color_name.to_ascii_lowercase().contains(&q)
                    || f.external_id.to_ascii_lowercase().contains(&q)
            })
            .take(limit)
            .collect()
    }
}

/// `SPOOLMAN_DB`, sibling `../SpoolmanDB`, then optional cache dirs.
pub fn spoolman_db_dir() -> Option<PathBuf> {
    if let Ok(p) = std::env::var("SPOOLMAN_DB") {
        let path = PathBuf::from(p);
        if is_spoolman_root(&path) {
            return Some(path);
        }
    }
    let manifest = PathBuf::from(env!("CARGO_MANIFEST_DIR"));
    let candidates = [
        PathBuf::from("/home/luluco/code/SpoolmanDB"),
        manifest.join("../../../SpoolmanDB"),
        manifest.join("../../SpoolmanDB"),
        PathBuf::from("..").join("SpoolmanDB"),
    ];
    candidates.into_iter().find(|p| is_spoolman_root(p))
}

pub fn load_default_catalog() -> CatalogIndex {
    match spoolman_db_dir() {
        Some(root) => load_catalog(&root).unwrap_or_default(),
        None => CatalogIndex::default(),
    }
}

pub fn load_catalog(root: impl AsRef<Path>) -> Result<CatalogIndex, ConfigError> {
    let root = root.as_ref();
    let mut index = CatalogIndex::default();
    let materials_path = root.join("materials.json");
    if materials_path.is_file() {
        index.materials = parse_materials(&std::fs::read_to_string(materials_path)?)?;
    }
    let filaments_dir = root.join("filaments");
    if filaments_dir.is_dir() {
        let mut files: Vec<PathBuf> = std::fs::read_dir(&filaments_dir)?
            .filter_map(|e| e.ok())
            .map(|e| e.path())
            .filter(|p| p.extension().and_then(|e| e.to_str()) == Some("json"))
            .collect();
        files.sort();
        for path in files {
            let text = std::fs::read_to_string(&path)?;
            index
                .filaments
                .extend(expand_vendor_json(&text, &index.materials)?);
        }
    }
    index.filaments.sort_by(|a, b| {
        (&a.manufacturer, &a.material, &a.name).cmp(&(&b.manufacturer, &b.material, &b.name))
    });
    Ok(index)
}

pub fn parse_materials(text: &str) -> Result<Vec<CatalogMaterial>, ConfigError> {
    let value: Value = serde_json::from_str(text)?;
    let Some(arr) = value.as_array() else {
        return Err(ConfigError::Message(
            "materials.json must be an array".into(),
        ));
    };
    let mut out = Vec::new();
    for item in arr {
        let Some(material) = item.get("material").and_then(Value::as_str) else {
            continue;
        };
        let density = item.get("density").and_then(Value::as_f64).unwrap_or(1.24);
        out.push(CatalogMaterial {
            material: material.to_string(),
            density,
            extruder_temp: intish(item.get("extruder_temp")),
            bed_temp: intish(item.get("bed_temp")),
        });
    }
    Ok(out)
}

pub fn expand_vendor_json(
    text: &str,
    materials: &[CatalogMaterial],
) -> Result<Vec<CatalogFilament>, ConfigError> {
    let file: VendorFile = serde_json::from_str(text)?;
    let mut out = Vec::new();
    for filament in file.filaments {
        out.extend(expand_one(&file.manufacturer, &filament, materials));
    }
    Ok(out)
}

/// SpoolmanDB compile id: `{manufacturer}_{material}_{name}_{weight}_{diameter}_{spooltype}`
/// with spaces stripped and non-ASCII dropped from the name.
pub fn catalog_id(
    manufacturer: &str,
    material: &str,
    name: &str,
    weight: f64,
    diameter: f64,
    spool_type: Option<&str>,
) -> String {
    let ascii_name: String = name.chars().filter(|c| c.is_ascii()).collect();
    let weight_s = format!("{weight:.0}");
    let diameter_s = format!("{diameter:.2}").replace('.', "");
    let spooltype_s = match spool_type {
        Some("plastic") => "p",
        Some("cardboard") => "c",
        Some("metal") => "m",
        _ => "n",
    };
    format!(
        "{}_{}_{}_{weight_s}_{diameter_s}_{spooltype_s}",
        manufacturer.to_ascii_lowercase(),
        material.to_ascii_lowercase(),
        ascii_name.to_ascii_lowercase(),
    )
    .replace(' ', "")
}

/// Overlay catalog physics onto settings (after an optional Generic BBL base).
pub fn apply_catalog_onto(settings: &mut SliceSettings, sku: &CatalogFilament) {
    settings.filament_vendor = sku.manufacturer.clone();
    settings.filament_type = sku.material.clone();
    if sku.density > 0.0 {
        settings.filament_density_g_cm3 = sku.density;
    }
    if sku.diameter > 0.0 {
        settings.filament_diameter_mm = sku.diameter;
    }
    if !sku.color_hex.is_empty() {
        settings.filament_colour = sku.color_hex.clone();
    }
    if let Some(temp) = sku.extruder_temp {
        settings.temperature_c = temp;
        settings.temperature_initial_layer_c = temp;
    }
    if let Some(bed) = sku.bed_temp {
        settings.bed_temperature_c = bed;
        settings.bed_temperature_initial_layer_c = bed;
        for plate in [
            &mut settings.cool_plate,
            &mut settings.eng_plate,
            &mut settings.hot_plate,
            &mut settings.textured_plate,
            &mut settings.supertack_plate,
        ] {
            plate.later_c = bed;
            plate.initial_c = bed;
        }
    }
    let note = format!("{} {}", sku.manufacturer, sku.name);
    if settings.filament_notes.is_empty() {
        settings.filament_notes = note;
    }
}

/// Overlay Generic `{material}` BBL when present, then catalog physics/temps/colour.
pub fn apply_sku_with_generic_base(
    settings: &mut SliceSettings,
    sku: &CatalogFilament,
    generic_path: Option<&Path>,
) -> Result<(), ConfigError> {
    if let Some(path) = generic_path {
        crate::overlay_bbl_profile(settings, path)?;
    }
    apply_catalog_onto(settings, sku);
    Ok(())
}

fn expand_one(
    manufacturer: &str,
    data: &RawFilament,
    materials: &[CatalogMaterial],
) -> Vec<CatalogFilament> {
    let mat = materials
        .iter()
        .find(|m| m.material.eq_ignore_ascii_case(&data.material));
    let mut out = Vec::new();
    for weight in &data.weights {
        for diameter in &data.diameters {
            for color in &data.colors {
                let color_name = color.name.clone();
                let formatted = data.name.replace("{color_name}", &color_name);
                let hex = color
                    .hex
                    .clone()
                    .or_else(|| color.hexes.as_ref().and_then(|h| h.first().cloned()))
                    .unwrap_or_default();
                let extruder = data.extruder_temp.or(mat.and_then(|m| m.extruder_temp));
                let bed = data.bed_temp.or(mat.and_then(|m| m.bed_temp));
                let density = if data.density > 0.0 {
                    data.density
                } else {
                    mat.map(|m| m.density).unwrap_or(1.24)
                };
                out.push(CatalogFilament {
                    external_id: catalog_id(
                        manufacturer,
                        &data.material,
                        &formatted,
                        weight.weight,
                        *diameter,
                        weight.spool_type.as_deref(),
                    ),
                    manufacturer: manufacturer.to_string(),
                    name: formatted,
                    material: data.material.clone(),
                    density,
                    diameter: *diameter,
                    net_weight: weight.weight,
                    spool_weight: weight.spool_weight.unwrap_or(0.0),
                    color_name,
                    color_hex: normalize_filament_colour(&hex),
                    extruder_temp: extruder,
                    bed_temp: bed,
                });
            }
        }
    }
    out
}

fn is_spoolman_root(path: &Path) -> bool {
    path.join("filaments").is_dir() || path.join("materials.json").is_file()
}

fn intish(v: Option<&Value>) -> Option<u16> {
    match v {
        Some(Value::Number(n)) => n.as_f64().map(|x| x.round().clamp(0.0, 500.0) as u16),
        Some(Value::String(s)) => s
            .parse::<f64>()
            .ok()
            .map(|x| x.round().clamp(0.0, 500.0) as u16),
        _ => None,
    }
}

#[derive(Debug, Deserialize)]
struct VendorFile {
    manufacturer: String,
    filaments: Vec<RawFilament>,
}

#[derive(Debug, Deserialize)]
struct RawFilament {
    name: String,
    material: String,
    #[serde(default)]
    density: f64,
    weights: Vec<RawWeight>,
    diameters: Vec<f64>,
    #[serde(default)]
    colors: Vec<RawColor>,
    #[serde(default)]
    extruder_temp: Option<u16>,
    #[serde(default)]
    bed_temp: Option<u16>,
}

#[derive(Debug, Deserialize)]
struct RawWeight {
    weight: f64,
    #[serde(default)]
    spool_weight: Option<f64>,
    #[serde(default)]
    spool_type: Option<String>,
}

#[derive(Debug, Deserialize)]
struct RawColor {
    name: String,
    #[serde(default)]
    hex: Option<String>,
    #[serde(default)]
    hexes: Option<Vec<String>>,
}

#[cfg(test)]
mod tests {
    use super::*;

    fn fixture_root() -> PathBuf {
        PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures/spoolman_db")
    }

    #[test]
    fn catalog_id_matches_python_formula() {
        assert_eq!(
            catalog_id("Bambu Lab", "PLA", "Jade White", 1000.0, 1.75, None),
            "bambulab_pla_jadewhite_1000_175_n"
        );
        assert_eq!(
            catalog_id(
                "Bambu Lab",
                "PLA",
                "Jade White",
                1000.0,
                1.75,
                Some("plastic")
            ),
            "bambulab_pla_jadewhite_1000_175_p"
        );
    }

    #[test]
    fn fixture_expands_jade_white_and_pla_density() {
        let index = load_catalog(fixture_root()).unwrap();
        let pla = index.material("PLA").expect("PLA");
        assert!((pla.density - 1.24).abs() < 1e-9);
        let sku = index
            .by_external_id("bambulab_pla_jadewhite_1000_175_n")
            .expect("jade white");
        assert_eq!(sku.manufacturer, "Bambu Lab");
        assert_eq!(sku.name, "Jade White");
        assert_eq!(sku.material, "PLA");
        assert!((sku.density - 1.24).abs() < 1e-9);
        assert!((sku.diameter - 1.75).abs() < 1e-9);
        assert_eq!(sku.color_hex, "#FFFFFFFF");
        assert_eq!(sku.extruder_temp, Some(220));
        assert_eq!(sku.bed_temp, Some(60));
        assert!((sku.net_weight - 1000.0).abs() < 1e-9);
        assert!((sku.spool_weight - 250.0).abs() < 1e-9);
    }

    #[test]
    fn apply_catalog_sets_physics_not_cube_defaults_vendor() {
        let index = load_catalog(fixture_root()).unwrap();
        let sku = index
            .by_external_id("bambulab_pla_jadewhite_1000_175_n")
            .unwrap();
        let mut s = SliceSettings::default();
        assert_eq!(s.filament_vendor, "Generic");
        apply_catalog_onto(&mut s, sku);
        assert_eq!(s.filament_vendor, "Bambu Lab");
        assert_eq!(s.filament_type, "PLA");
        assert_eq!(s.temperature_c, 220);
        assert_eq!(s.bed_temperature_c, 60);
        assert!((s.filament_density_g_cm3 - 1.24).abs() < 1e-9);
        let empty = SliceSettings::default();
        assert_eq!(empty.filament_vendor, "Generic");
        assert!((empty.filament_density_g_cm3 - 1.24).abs() < 1e-9);
    }

    #[test]
    fn search_caps_results() {
        let index = load_catalog(fixture_root()).unwrap();
        assert_eq!(index.search("jade", 8).len(), 1);
        assert!(index.search("petg", 8).is_empty());
    }
}
