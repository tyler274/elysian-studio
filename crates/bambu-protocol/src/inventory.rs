//! Local Spoolman-style inventory: vendor → filament → spool.
//!
//! Persists `filament_inventory/inventory.json`. Migrates the older flat
//! `spools.json` on first load.

use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};

use crate::spools::{parse_spools_file, FilamentSpool};

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, Default)]
pub struct Inventory {
    #[serde(default)]
    pub vendors: Vec<InventoryVendor>,
    #[serde(default)]
    pub filaments: Vec<InventoryFilament>,
    #[serde(default)]
    pub spools: Vec<InventorySpool>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, Default)]
pub struct InventoryVendor {
    pub id: String,
    pub name: String,
    #[serde(default)]
    pub empty_spool_weight: f64,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct InventoryFilament {
    pub id: String,
    #[serde(default)]
    pub vendor_id: String,
    #[serde(default)]
    pub name: String,
    #[serde(default)]
    pub material: String,
    #[serde(default = "default_density")]
    pub density: f64,
    #[serde(default = "default_diameter")]
    pub diameter: f64,
    #[serde(default)]
    pub weight: f64,
    #[serde(default)]
    pub spool_weight: f64,
    #[serde(default)]
    pub external_id: String,
    #[serde(default)]
    pub color_hex: String,
    #[serde(default)]
    pub color_name: String,
    #[serde(default)]
    pub extruder_temp: Option<u16>,
    #[serde(default)]
    pub bed_temp: Option<u16>,
    #[serde(default)]
    pub bambu_filament_id: String,
}

impl Default for InventoryFilament {
    fn default() -> Self {
        Self {
            id: String::new(),
            vendor_id: String::new(),
            name: String::from("PLA"),
            material: String::from("PLA"),
            density: 1.24,
            diameter: 1.75,
            weight: 1000.0,
            spool_weight: 0.0,
            external_id: String::new(),
            color_hex: String::from("#FFFFFFFF"),
            color_name: String::new(),
            extruder_temp: None,
            bed_temp: None,
            bambu_filament_id: String::new(),
        }
    }
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct InventorySpool {
    pub id: String,
    pub filament_id: String,
    #[serde(default)]
    pub used_weight: f64,
    #[serde(default)]
    pub initial_weight: f64,
    #[serde(default)]
    pub spool_weight: f64,
    #[serde(default)]
    pub location: String,
    #[serde(default)]
    pub archived: bool,
    #[serde(default)]
    pub comment: String,
    #[serde(default)]
    pub cloud_id: String,
    #[serde(default)]
    pub cloud_synced: bool,
    #[serde(default)]
    pub ams_id: i32,
    #[serde(default = "unset_slot")]
    pub slot_id: i32,
}

impl Default for InventorySpool {
    fn default() -> Self {
        Self {
            id: String::new(),
            filament_id: String::new(),
            used_weight: 0.0,
            initial_weight: 1000.0,
            spool_weight: 0.0,
            location: String::new(),
            archived: false,
            comment: String::new(),
            cloud_id: String::new(),
            cloud_synced: false,
            ams_id: -1,
            slot_id: -1,
        }
    }
}

fn default_density() -> f64 {
    1.24
}

fn default_diameter() -> f64 {
    1.75
}

fn unset_slot() -> i32 {
    -1
}

/// C++ / Spoolman: remaining grams on the spool.
pub fn remaining_weight(spool: &InventorySpool, filament: Option<&InventoryFilament>) -> f64 {
    (initial_weight(spool, filament) - spool.used_weight).max(0.0)
}

pub fn initial_weight(spool: &InventorySpool, filament: Option<&InventoryFilament>) -> f64 {
    if spool.initial_weight > 0.0 {
        spool.initial_weight
    } else {
        filament.map(|f| f.weight).unwrap_or(0.0)
    }
}

pub fn spool_tare(spool: &InventorySpool, filament: Option<&InventoryFilament>) -> f64 {
    if spool.spool_weight > 0.0 {
        spool.spool_weight
    } else {
        filament.map(|f| f.spool_weight).unwrap_or(0.0)
    }
}

pub fn remain_percent(spool: &InventorySpool, filament: Option<&InventoryFilament>) -> i32 {
    let initial = initial_weight(spool, filament);
    if initial <= 0.0 {
        return 0;
    }
    let pct = remaining_weight(spool, filament) / initial * 100.0;
    pct.round().clamp(0.0, 100.0) as i32
}

/// Spoolman `length_from_weight`: length in mm from grams.
pub fn length_from_weight(weight_g: f64, diameter_mm: f64, density: f64) -> f64 {
    if density <= 0.0 || diameter_mm <= 0.0 {
        return 0.0;
    }
    let volume_cm3 = weight_g / density;
    let volume_mm3 = volume_cm3 * 1000.0;
    volume_mm3 / (std::f64::consts::PI * (diameter_mm * 0.5).powi(2))
}

/// Spoolman `weight_from_length`: grams from length in mm.
pub fn weight_from_length(length_mm: f64, diameter_mm: f64, density: f64) -> f64 {
    if density <= 0.0 || diameter_mm <= 0.0 {
        return 0.0;
    }
    let volume_mm3 = length_mm * std::f64::consts::PI * (diameter_mm * 0.5).powi(2);
    let volume_cm3 = volume_mm3 / 1000.0;
    density * volume_cm3
}

impl Inventory {
    pub fn filament(&self, id: &str) -> Option<&InventoryFilament> {
        self.filaments.iter().find(|f| f.id == id)
    }

    pub fn filament_mut(&mut self, id: &str) -> Option<&mut InventoryFilament> {
        self.filaments.iter_mut().find(|f| f.id == id)
    }

    pub fn vendor(&self, id: &str) -> Option<&InventoryVendor> {
        self.vendors.iter().find(|v| v.id == id)
    }

    pub fn spool(&self, id: &str) -> Option<&InventorySpool> {
        self.spools.iter().find(|s| s.id == id)
    }

    pub fn spool_mut(&mut self, id: &str) -> Option<&mut InventorySpool> {
        self.spools.iter_mut().find(|s| s.id == id)
    }

    pub fn flatten(&self, spool: &InventorySpool) -> FilamentSpool {
        let filament = self.filament(&spool.filament_id);
        let vendor = filament.and_then(|f| self.vendor(&f.vendor_id));
        let remaining = remaining_weight(spool, filament);
        let initial = initial_weight(spool, filament);
        FilamentSpool {
            spool_id: if spool.cloud_id.is_empty() {
                spool.id.clone()
            } else {
                spool.cloud_id.clone()
            },
            filament_id: filament
                .map(|f| f.bambu_filament_id.clone())
                .unwrap_or_default(),
            brand: vendor.map(|v| v.name.clone()).unwrap_or_default(),
            material_type: filament
                .map(|f| f.material.clone())
                .unwrap_or_else(|| String::from("PLA")),
            series: filament.map(|f| f.name.clone()).unwrap_or_default(),
            color_name: filament.map(|f| f.color_name.clone()).unwrap_or_default(),
            color_code: filament
                .map(|f| f.color_hex.clone())
                .unwrap_or_else(|| String::from("#FFFFFFFF")),
            diameter: filament.map(|f| f.diameter).unwrap_or(1.75),
            initial_weight: initial,
            spool_weight: spool_tare(spool, filament),
            net_weight: remaining,
            remain_percent: remain_percent(spool, filament),
            note: spool.comment.clone(),
            status: if spool.archived {
                String::from("archived")
            } else {
                String::from("active")
            },
            cloud_synced: spool.cloud_synced,
            ams_id: spool.ams_id,
            slot_id: spool.slot_id,
            ..FilamentSpool::default()
        }
    }

    pub fn ensure_vendor(&mut self, name: &str) -> String {
        if name.is_empty() {
            return String::new();
        }
        if let Some(v) = self
            .vendors
            .iter()
            .find(|v| v.name.eq_ignore_ascii_case(name))
        {
            return v.id.clone();
        }
        let id = format!("v{}", self.vendors.len() + 1);
        self.vendors.push(InventoryVendor {
            id: id.clone(),
            name: name.to_string(),
            empty_spool_weight: 0.0,
        });
        id
    }

    #[allow(clippy::too_many_arguments)]
    pub fn add_catalog_sku(
        &mut self,
        external_id: &str,
        manufacturer: &str,
        name: &str,
        material: &str,
        density: f64,
        diameter: f64,
        net_weight: f64,
        spool_weight: f64,
        color_name: &str,
        color_hex: &str,
        extruder_temp: Option<u16>,
        bed_temp: Option<u16>,
    ) -> String {
        let vendor_id = self.ensure_vendor(manufacturer);
        if let Some(existing) = self
            .filaments
            .iter()
            .find(|f| !external_id.is_empty() && f.external_id == external_id)
        {
            return existing.id.clone();
        }
        let id = format!("f{}", self.filaments.len() + 1);
        self.filaments.push(InventoryFilament {
            id: id.clone(),
            vendor_id,
            name: name.to_string(),
            material: material.to_string(),
            density,
            diameter,
            weight: net_weight,
            spool_weight,
            external_id: external_id.to_string(),
            color_hex: color_hex.to_string(),
            color_name: color_name.to_string(),
            extruder_temp,
            bed_temp,
            bambu_filament_id: String::new(),
        });
        id
    }

    pub fn add_spool_for_filament(&mut self, filament_id: &str) -> String {
        let filament = self.filament(filament_id).cloned();
        let id = format!("s{}", self.spools.len() + 1);
        self.spools.push(InventorySpool {
            id: id.clone(),
            filament_id: filament_id.to_string(),
            used_weight: 0.0,
            initial_weight: filament.as_ref().map(|f| f.weight).unwrap_or(1000.0),
            spool_weight: filament.as_ref().map(|f| f.spool_weight).unwrap_or(0.0),
            ..InventorySpool::default()
        });
        id
    }

    pub fn use_grams(&mut self, spool_id: &str, grams: f64) {
        if let Some(spool) = self.spool_mut(spool_id) {
            spool.used_weight = (spool.used_weight + grams.max(0.0)).max(0.0);
        }
    }

    /// Set remaining plastic from a scale reading of the full spool (gross grams).
    pub fn measure_gross(&mut self, spool_id: &str, gross_g: f64) {
        let filament_id = self.spool(spool_id).map(|s| s.filament_id.clone());
        let filament = filament_id
            .as_deref()
            .and_then(|id| self.filament(id).cloned());
        let tare = self
            .spool(spool_id)
            .map(|s| spool_tare(s, filament.as_ref()))
            .unwrap_or(0.0);
        let remaining = (gross_g - tare).max(0.0);
        if let Some(spool) = self.spool_mut(spool_id) {
            let initial = initial_weight(spool, filament.as_ref());
            spool.used_weight = (initial - remaining).max(0.0);
        }
    }

    pub fn upsert_flat(&mut self, flat: &FilamentSpool) -> String {
        let vendor_id = self.ensure_vendor(&flat.brand);
        let filament_id = if let Some(existing) = self.filaments.iter().find(|f| {
            (!flat.filament_id.is_empty() && f.bambu_filament_id == flat.filament_id)
                || (f.name == flat.series
                    && f.material == flat.material_type
                    && f.color_hex == flat.color_code
                    && f.vendor_id == vendor_id)
        }) {
            existing.id.clone()
        } else {
            let id = format!("f{}", self.filaments.len() + 1);
            self.filaments.push(InventoryFilament {
                id: id.clone(),
                vendor_id,
                name: if flat.series.is_empty() {
                    flat.material_type.clone()
                } else {
                    flat.series.clone()
                },
                material: flat.material_type.clone(),
                density: 1.24,
                diameter: flat.diameter,
                weight: if flat.initial_weight > 0.0 {
                    flat.initial_weight
                } else {
                    1000.0
                },
                spool_weight: flat.spool_weight,
                color_hex: flat.color_code.clone(),
                color_name: flat.color_name.clone(),
                bambu_filament_id: flat.filament_id.clone(),
                ..InventoryFilament::default()
            });
            id
        };
        if let Some(existing) = self.spools.iter_mut().find(|s| {
            !flat.spool_id.is_empty() && (s.id == flat.spool_id || s.cloud_id == flat.spool_id)
        }) {
            existing.filament_id = filament_id;
            existing.comment = flat.note.clone();
            existing.cloud_synced = flat.cloud_synced;
            if !flat.spool_id.is_empty() {
                existing.cloud_id = flat.spool_id.clone();
            }
            existing.ams_id = flat.ams_id;
            existing.slot_id = flat.slot_id;
            existing.archived = flat.status.eq_ignore_ascii_case("archived");
            let initial = if flat.initial_weight > 0.0 {
                flat.initial_weight
            } else {
                existing.initial_weight
            };
            existing.initial_weight = initial;
            existing.used_weight = (initial - flat.net_weight).max(0.0);
            return existing.id.clone();
        }
        let id = format!("s{}", self.spools.len() + 1);
        let initial = if flat.initial_weight > 0.0 {
            flat.initial_weight
        } else {
            1000.0
        };
        self.spools.push(InventorySpool {
            id: id.clone(),
            filament_id,
            used_weight: (initial - flat.net_weight).max(0.0),
            initial_weight: initial,
            spool_weight: flat.spool_weight,
            comment: flat.note.clone(),
            cloud_id: flat.spool_id.clone(),
            cloud_synced: flat.cloud_synced,
            ams_id: flat.ams_id,
            slot_id: flat.slot_id,
            archived: flat.status.eq_ignore_ascii_case("archived"),
            ..InventorySpool::default()
        });
        id
    }

    pub fn delete_spool(&mut self, id: &str) {
        self.spools.retain(|s| s.id != id);
    }
}

pub fn inventory_path(config_dir: impl AsRef<Path>) -> PathBuf {
    crate::spools::inventory_dir(config_dir).join("inventory.json")
}

pub fn load_inventory(config_dir: impl AsRef<Path>) -> Result<Inventory, std::io::Error> {
    let path = inventory_path(&config_dir);
    if path.is_file() {
        let text = std::fs::read_to_string(&path)?;
        return serde_json::from_str(&text)
            .map_err(|err| std::io::Error::new(std::io::ErrorKind::InvalidData, err));
    }
    let legacy = crate::spools::spools_path(&config_dir);
    if legacy.is_file() {
        let text = std::fs::read_to_string(legacy)?;
        let flats = parse_spools_file(&text)
            .map_err(|err| std::io::Error::new(std::io::ErrorKind::InvalidData, err))?;
        let mut inv = Inventory::default();
        for flat in &flats {
            inv.upsert_flat(flat);
        }
        let _ = save_inventory(&config_dir, &inv);
        return Ok(inv);
    }
    Ok(Inventory::default())
}

pub fn save_inventory(
    config_dir: impl AsRef<Path>,
    inventory: &Inventory,
) -> Result<(), std::io::Error> {
    let dir = crate::spools::inventory_dir(&config_dir);
    std::fs::create_dir_all(&dir)?;
    let json = serde_json::to_vec_pretty(inventory)
        .map_err(|err| std::io::Error::new(std::io::ErrorKind::InvalidData, err))?;
    std::fs::write(inventory_path(config_dir), json)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn remaining_math_matches_spoolman() {
        let filament = InventoryFilament {
            weight: 1000.0,
            density: 1.24,
            diameter: 1.75,
            ..InventoryFilament::default()
        };
        let spool = InventorySpool {
            used_weight: 180.0,
            initial_weight: 1000.0,
            ..InventorySpool::default()
        };
        assert!((remaining_weight(&spool, Some(&filament)) - 820.0).abs() < 1e-9);
        assert_eq!(remain_percent(&spool, Some(&filament)), 82);
        let len = length_from_weight(1.24, 1.75, 1.24);
        let back = weight_from_length(len, 1.75, 1.24);
        assert!((back - 1.24).abs() < 1e-6);
    }

    #[test]
    fn migrate_spools_json_to_inventory() {
        let dir = std::env::temp_dir().join(format!("bambu-rs-inv-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        let mut spool = FilamentSpool::default();
        spool.spool_id = "42".into();
        spool.filament_id = "GFA00".into();
        spool.brand = "Bambu Lab".into();
        spool.series = "Jade White".into();
        spool.material_type = "PLA".into();
        spool.initial_weight = 1000.0;
        spool.net_weight = 820.0;
        crate::save_spools(&dir, &[spool]).unwrap();
        let inv = load_inventory(&dir).unwrap();
        assert_eq!(inv.spools.len(), 1);
        assert_eq!(inv.vendors[0].name, "Bambu Lab");
        assert_eq!(inv.filaments[0].bambu_filament_id, "GFA00");
        assert!((inv.spools[0].used_weight - 180.0).abs() < 1e-9);
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn measure_gross_sets_used_weight() {
        let mut inv = Inventory::default();
        let fid = inv.add_catalog_sku(
            "bambulab_pla_jadewhite_1000_175_n",
            "Bambu Lab",
            "Jade White",
            "PLA",
            1.24,
            1.75,
            1000.0,
            250.0,
            "Jade White",
            "#FFFFFFFF",
            Some(220),
            Some(60),
        );
        let sid = inv.add_spool_for_filament(&fid);
        inv.measure_gross(&sid, 1070.0);
        let spool = inv.spool(&sid).unwrap();
        assert!((remaining_weight(spool, inv.filament(&fid)) - 820.0).abs() < 1e-6);
    }
}
