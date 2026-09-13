//! Local `filament_inventory/spools.json` plus cloud FilamentV2 JSON helpers.
//!
//! Persistence is real (C++ `load`/`save` are stubs). Cloud HTTP stays on
//! [`crate::CloudApi`]; this module only parses and builds JSON.

use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};
use serde_json::{json, Value};

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct FilamentSpool {
    #[serde(default)]
    pub spool_id: String,
    #[serde(default, alias = "setting_id")]
    pub filament_id: String,
    #[serde(default)]
    pub tag_uid: String,
    #[serde(default)]
    pub tray_id_name: String,
    #[serde(default)]
    pub brand: String,
    #[serde(default)]
    pub material_type: String,
    #[serde(default)]
    pub series: String,
    #[serde(default)]
    pub color_name: String,
    #[serde(default)]
    pub color_code: String,
    #[serde(default)]
    pub colors: Vec<String>,
    /// 0 gradient / 1 multi / 2 single (C++ `color_type`).
    #[serde(default = "default_color_type")]
    pub color_type: i32,
    #[serde(default = "default_diameter")]
    pub diameter: f64,
    #[serde(default)]
    pub initial_weight: f64,
    #[serde(default)]
    pub spool_weight: f64,
    #[serde(default)]
    pub net_weight: f64,
    #[serde(default)]
    pub remain_percent: i32,
    #[serde(default)]
    pub status: String,
    #[serde(default)]
    pub entry_method: String,
    #[serde(default)]
    pub bound_dev_id: String,
    #[serde(default)]
    pub bound_ams_id: String,
    #[serde(default)]
    pub in_printer: bool,
    #[serde(default)]
    pub dev_id: String,
    #[serde(default)]
    pub ams_id: i32,
    #[serde(default = "unset_slot")]
    pub slot_id: i32,
    #[serde(default)]
    pub note: String,
    #[serde(default)]
    pub favorite: bool,
    #[serde(default)]
    pub cloud_synced: bool,
}

fn default_color_type() -> i32 {
    2
}

fn default_diameter() -> f64 {
    1.75
}

fn unset_slot() -> i32 {
    -1
}

impl Default for FilamentSpool {
    fn default() -> Self {
        Self {
            spool_id: String::new(),
            filament_id: String::new(),
            tag_uid: String::new(),
            tray_id_name: String::new(),
            brand: String::new(),
            material_type: String::from("PLA"),
            series: String::new(),
            color_name: String::new(),
            color_code: String::from("#FFFFFFFF"),
            colors: Vec::new(),
            color_type: 2,
            diameter: 1.75,
            initial_weight: 1000.0,
            spool_weight: 0.0,
            net_weight: 1000.0,
            remain_percent: 100,
            status: String::from("active"),
            entry_method: String::from("manual"),
            bound_dev_id: String::new(),
            bound_ams_id: String::new(),
            in_printer: false,
            dev_id: String::new(),
            ams_id: -1,
            slot_id: -1,
            note: String::new(),
            favorite: false,
            cloud_synced: false,
        }
    }
}

impl FilamentSpool {
    pub fn label(&self) -> String {
        let name = if self.series.is_empty() {
            self.material_type.as_str()
        } else {
            self.series.as_str()
        };
        let brand = if self.brand.is_empty() {
            "—"
        } else {
            self.brand.as_str()
        };
        format!(
            "{brand} {name} {} · {}%",
            self.color_code, self.remain_percent
        )
    }
}

pub fn inventory_dir(config_dir: impl AsRef<Path>) -> PathBuf {
    config_dir.as_ref().join("filament_inventory")
}

pub fn spools_path(config_dir: impl AsRef<Path>) -> PathBuf {
    inventory_dir(config_dir).join("spools.json")
}

pub fn load_spools(config_dir: impl AsRef<Path>) -> Result<Vec<FilamentSpool>, std::io::Error> {
    let path = spools_path(config_dir);
    if !path.is_file() {
        return Ok(Vec::new());
    }
    let text = std::fs::read_to_string(path)?;
    parse_spools_file(&text)
        .map_err(|err| std::io::Error::new(std::io::ErrorKind::InvalidData, err))
}

pub fn save_spools(
    config_dir: impl AsRef<Path>,
    spools: &[FilamentSpool],
) -> Result<(), std::io::Error> {
    let dir = inventory_dir(&config_dir);
    std::fs::create_dir_all(&dir)?;
    let path = dir.join("spools.json");
    let json = serde_json::to_vec_pretty(spools)
        .map_err(|err| std::io::Error::new(std::io::ErrorKind::InvalidData, err))?;
    std::fs::write(path, json)
}

pub fn parse_spools_file(text: &str) -> Result<Vec<FilamentSpool>, serde_json::Error> {
    let value: Value = serde_json::from_str(text)?;
    if let Ok(list) = serde_json::from_value::<Vec<FilamentSpool>>(value.clone()) {
        return Ok(list);
    }
    if let Some(arr) = value.get("spools").cloned() {
        return serde_json::from_value(arr);
    }
    Ok(Vec::new())
}

pub fn filament_v2_path() -> &'static str {
    "/my/filament/v2"
}

pub fn filament_v2_batch_path() -> &'static str {
    "/my/filament/v2/batch"
}

pub fn filament_v2_ams_sync_path() -> &'static str {
    "/my/filament/v2/ams/sync"
}

/// Cloud list payload: `data.filaments` or a top-level array.
pub fn parse_cloud_filaments(v: &Value) -> Vec<FilamentSpool> {
    let arrays = [
        v.get("filaments"),
        v.pointer("/data/filaments"),
        v.get("list"),
        v.pointer("/data/list"),
        v.as_array().map(|_| v),
    ];
    for arr in arrays.into_iter().flatten() {
        if let Some(items) = arr.as_array() {
            return items.iter().filter_map(spool_from_cloud_json).collect();
        }
    }
    Vec::new()
}

#[allow(clippy::field_reassign_with_default)]
pub fn spool_from_cloud_json(v: &Value) -> Option<FilamentSpool> {
    let mut s = FilamentSpool::default();
    s.spool_id = textish(v, &["id", "spool_id"]);
    s.filament_id = textish(v, &["filamentId", "setting_id"]);
    s.tag_uid = textish(v, &["RFID", "rfid", "tag_uid"]);
    s.tray_id_name = textish(v, &["trayIdName", "tray_id_name"]);
    s.brand = textish(v, &["filamentVendor", "brand"]);
    s.material_type = textish(v, &["filamentType", "material_type"]);
    s.series = textish(v, &["filamentName", "series"]);
    s.color_code = textish(v, &["color", "color_code"]);
    s.color_name = textish(v, &["color_name"]);
    s.note = textish(v, &["note"]);
    if let Some(Value::Array(colors)) = v.get("colors") {
        s.colors = colors
            .iter()
            .filter_map(Value::as_str)
            .map(str::to_string)
            .collect();
    }
    s.color_type = intish(v, &["colorType", "color_type"]).unwrap_or(2);
    s.diameter = numish(v, &["diameter"]).unwrap_or(1.75);
    s.initial_weight = numish(v, &["totalNetWeight", "initial_weight"]).unwrap_or(0.0);
    s.net_weight = numish(v, &["netWeight", "net_weight"]).unwrap_or(0.0);
    s.spool_weight = numish(v, &["spool_weight"]).unwrap_or(0.0);
    if s.initial_weight > 0.0 && s.net_weight >= 0.0 {
        let pct = (s.net_weight / s.initial_weight * 100.0).clamp(0.0, 100.0);
        s.remain_percent = (pct + 0.5) as i32;
    }
    s.in_printer = boolish(v, &["inPrinter", "in_printer"]);
    s.dev_id = textish(v, &["devId", "dev_id"]);
    s.ams_id = intish(v, &["amsId", "ams_id"]).unwrap_or(-1);
    s.slot_id = intish(v, &["slotId", "slot_id"]).unwrap_or(-1);
    s.cloud_synced = true;
    if s.spool_id.is_empty() && s.filament_id.is_empty() && s.brand.is_empty() {
        return None;
    }
    Some(s)
}

pub fn spool_to_cloud_json(s: &FilamentSpool) -> Value {
    let create_type = if s.tag_uid.is_empty() {
        "manual"
    } else {
        "ams"
    };
    let mut body = json!({
        "createType": create_type,
        "filamentVendor": s.brand,
        "filamentType": s.material_type,
        "filamentName": s.series,
        "filamentId": s.filament_id,
        "isSupport": false,
        "color": s.color_code,
        "colorType": s.color_type,
        "trayIdName": s.tray_id_name,
        "netWeight": s.net_weight.round() as i64,
        "totalNetWeight": s.initial_weight.round() as i64,
        "inPrinter": s.in_printer,
    });
    if !s.spool_id.is_empty() {
        body["id"] = json!(s.spool_id);
    }
    if !s.tag_uid.is_empty() {
        body["RFID"] = json!(s.tag_uid);
    }
    if !s.colors.is_empty() {
        body["colors"] = json!(s.colors);
    }
    if !s.note.is_empty() {
        body["note"] = json!(s.note);
    }
    if !s.dev_id.is_empty() {
        body["devId"] = json!(s.dev_id);
    }
    if s.ams_id >= 0 {
        body["amsId"] = json!(s.ams_id);
    }
    if s.slot_id >= 0 {
        body["slotId"] = json!(s.slot_id);
    }
    body
}

pub fn batch_delete_body(ids: &[String]) -> Value {
    json!({ "ids": ids })
}

fn textish(v: &Value, keys: &[&str]) -> String {
    for key in keys {
        match v.get(*key) {
            Some(Value::String(s)) if !s.is_empty() => return s.clone(),
            Some(Value::Number(n)) => return n.to_string(),
            _ => {}
        }
    }
    String::new()
}

fn numish(v: &Value, keys: &[&str]) -> Option<f64> {
    for key in keys {
        match v.get(*key) {
            Some(Value::Number(n)) => return n.as_f64(),
            Some(Value::String(s)) => return s.parse().ok(),
            _ => {}
        }
    }
    None
}

fn intish(v: &Value, keys: &[&str]) -> Option<i32> {
    numish(v, keys).map(|n| n.round() as i32)
}

fn boolish(v: &Value, keys: &[&str]) -> bool {
    for key in keys {
        match v.get(*key) {
            Some(Value::Bool(b)) => return *b,
            Some(Value::Number(n)) => return n.as_i64().unwrap_or(0) != 0,
            Some(Value::String(s)) => {
                return matches!(s.to_ascii_lowercase().as_str(), "1" | "true" | "yes")
            }
            _ => {}
        }
    }
    false
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn cloud_roundtrip_fixture() {
        let v = json!({
            "data": {
                "filaments": [{
                    "id": "42",
                    "filamentId": "GFA00",
                    "filamentVendor": "Bambu Lab",
                    "filamentType": "PLA",
                    "filamentName": "PLA Basic",
                    "color": "#00AE42",
                    "colorType": 2,
                    "totalNetWeight": 1000,
                    "netWeight": 820,
                    "note": "kitchen"
                }]
            }
        });
        let list = parse_cloud_filaments(&v);
        assert_eq!(list.len(), 1);
        assert_eq!(list[0].spool_id, "42");
        assert_eq!(list[0].filament_id, "GFA00");
        assert_eq!(list[0].brand, "Bambu Lab");
        assert_eq!(list[0].remain_percent, 82);
        let body = spool_to_cloud_json(&list[0]);
        assert_eq!(body["filamentVendor"], "Bambu Lab");
        assert_eq!(body["filamentId"], "GFA00");
        assert_eq!(body["id"], "42");
        assert_eq!(body["createType"], "manual");
    }

    #[test]
    fn local_spools_json_roundtrip() {
        let dir = std::env::temp_dir().join(format!("bambu-rs-spools-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        let mut spool = FilamentSpool::default();
        spool.spool_id = "local-1".into();
        spool.filament_id = "GFA00".into();
        spool.brand = "Bambu Lab".into();
        spool.series = "PLA Basic".into();
        save_spools(&dir, &[spool.clone()]).unwrap();
        let loaded = load_spools(&dir).unwrap();
        assert_eq!(loaded.len(), 1);
        assert_eq!(loaded[0].filament_id, "GFA00");
        let _ = std::fs::remove_dir_all(&dir);
    }
}
