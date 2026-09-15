//! Public HMS catalog (`https://e.bambulab.com/query.php`) — no plugin, no PEMs.

use std::path::{Path, PathBuf};

use bambu_device::HmsCode;
use serde_json::Value;
use thiserror::Error;

use crate::https::{self, HttpsError};

pub const HMS_HOST: &str = "e.bambulab.com";

#[derive(Debug, Error)]
pub enum HmsError {
    #[error("hms: {0}")]
    Message(String),
    #[error(transparent)]
    Https(#[from] HttpsError),
    #[error(transparent)]
    Io(#[from] std::io::Error),
    #[error(transparent)]
    Json(#[from] serde_json::Error),
}

pub fn catalog_cache_dir(config_dir: impl AsRef<Path>) -> PathBuf {
    config_dir.as_ref().join("hms")
}

pub fn catalog_cache_path(config_dir: impl AsRef<Path>, lang: &str) -> PathBuf {
    catalog_cache_dir(config_dir).join(format!("hms_{lang}.json"))
}

pub fn load_cached_catalog(config_dir: impl AsRef<Path>, lang: &str) -> Option<Value> {
    let path = catalog_cache_path(config_dir, lang);
    let text = std::fs::read_to_string(path).ok()?;
    serde_json::from_str(&text).ok()
}

pub fn save_cached_catalog(
    config_dir: impl AsRef<Path>,
    lang: &str,
    catalog: &Value,
) -> Result<PathBuf, HmsError> {
    let dir = catalog_cache_dir(&config_dir);
    std::fs::create_dir_all(&dir)?;
    let path = catalog_cache_path(config_dir, lang);
    std::fs::write(&path, serde_json::to_vec_pretty(catalog)?)?;
    Ok(path)
}

/// C++ `HMSQuery::_query_hms_msg`: `device_hms[lang][].ecode` → `intro`.
pub fn lookup_hms_intro(catalog: &Value, long_error_code: &str, lang: &str) -> Option<String> {
    let want = long_error_code.to_ascii_uppercase();
    let data = catalog.get("data").unwrap_or(catalog);
    let device_hms = data.get("device_hms")?;
    let langs: Vec<&Value> = if lang.is_empty() {
        match device_hms {
            Value::Object(map) => map.values().collect(),
            _ => vec![device_hms],
        }
    } else {
        device_hms
            .get(lang)
            .or_else(|| device_hms.get("en"))
            .into_iter()
            .collect()
    };
    for list in langs {
        let Some(arr) = list.as_array() else {
            continue;
        };
        for item in arr {
            let Some(code) = item.get("ecode").and_then(Value::as_str) else {
                continue;
            };
            if code.eq_ignore_ascii_case(&want) {
                if let Some(intro) = item.get("intro").and_then(Value::as_str) {
                    if !intro.is_empty() {
                        return Some(intro.to_string());
                    }
                }
            }
        }
    }
    None
}

pub fn describe_hms(catalog: Option<&Value>, code: HmsCode, lang: &str) -> String {
    let long = code.long_error_code();
    catalog
        .and_then(|c| lookup_hms_intro(c, &long, lang))
        .map(|intro| format!("{intro} [{long}]"))
        .unwrap_or(long)
}

/// HTTPS GET `https://e.bambulab.com/query.php?lang=…` (C++ `HMSQuery::download_hms_related`).
pub fn fetch_catalog(lang: &str) -> Result<Value, HmsError> {
    let lang = if lang.is_empty() { "en" } else { lang };
    let path = format!("/query.php?lang={lang}");
    let resp = https::request(
        "GET",
        HMS_HOST,
        &path,
        &[("Accept", "application/json")],
        None,
    )?;
    if resp.status != 200 {
        return Err(HmsError::Message(format!("HMS HTTP {}", resp.status)));
    }
    let value: Value = serde_json::from_str(&resp.body_text())?;
    if value.get("result").and_then(Value::as_i64) == Some(0) {
        if let Some(data) = value.get("data") {
            return Ok(data.clone());
        }
    }
    Ok(value)
}

pub fn refresh_catalog(config_dir: impl AsRef<Path>, lang: &str) -> Result<Value, HmsError> {
    let catalog = fetch_catalog(lang)?;
    save_cached_catalog(config_dir, lang, &catalog)?;
    Ok(catalog)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn looks_up_intro_from_catalog() {
        let catalog = serde_json::json!({
            "device_hms": {
                "en": [
                    {"ecode": "0700010000010001", "intro": "Filament runout"}
                ]
            }
        });
        let code = HmsCode {
            attr: 0x0700_0100,
            code: 0x0001_0001,
        };
        assert_eq!(
            describe_hms(Some(&catalog), code, "en"),
            "Filament runout [0700010000010001]"
        );
        assert_eq!(describe_hms(None, code, "en"), "0700010000010001");
    }

    #[test]
    fn cache_roundtrip() {
        let dir = std::env::temp_dir().join(format!("bambu-hms-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        let catalog = serde_json::json!({"device_hms": {"en": []}});
        save_cached_catalog(&dir, "en", &catalog).unwrap();
        let loaded = load_cached_catalog(&dir, "en").unwrap();
        assert_eq!(loaded, catalog);
        let _ = std::fs::remove_dir_all(&dir);
    }
}
