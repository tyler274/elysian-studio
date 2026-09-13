//! Import LAN codes, user id, and optional cloud tokens from an existing Studio data dir.
//!
//! The C++ Studio tree is a config source only. PEMs are copied or scanned from a
//! local plugin blob and written under `$XDG_CONFIG_HOME/bambu-studio-rs`.

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};

use serde_json::Value;

use crate::cloud::{save_cloud_session, CloudSession};
use crate::credentials::{
    candidate_import_dirs, default_config_dir, write_to_dir, CredentialError,
};
use crate::extract::{extract_to_config_dir, ExtractReport};

const LAN_CODES_FILE: &str = "lan_access_codes.json";

#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct StudioPrinter {
    pub serial: String,
    pub access_code: String,
}

#[derive(Debug, Clone, Default)]
pub struct StudioImport {
    pub user_id: String,
    pub region: String,
    pub printers: Vec<StudioPrinter>,
    pub default_serial: String,
    pub has_token: bool,
    pub has_refresh: bool,
    pub extract: ExtractReport,
    pub notes: Vec<String>,
}

impl StudioImport {
    pub fn status_lines(&self) -> Vec<String> {
        let mut lines = vec![
            format!(
                "cloud_user: {}",
                if self.user_id.is_empty() {
                    "missing"
                } else {
                    "present"
                }
            ),
            format!(
                "cloud_token: {}",
                if self.has_token { "present" } else { "missing" }
            ),
            format!(
                "cloud_refresh: {}",
                if self.has_refresh {
                    "present"
                } else {
                    "missing"
                }
            ),
            format!(
                "cloud_region: {}",
                if self.region.is_empty() {
                    "us (default)"
                } else {
                    self.region.as_str()
                }
            ),
            format!("LAN codes: {} printer(s)", self.printers.len()),
        ];
        lines.extend(self.extract.credentials.status_lines());
        lines.extend(self.extract.notes.iter().cloned());
        lines.extend(self.notes.iter().cloned());
        lines
    }
}

pub fn default_studio_data_dir() -> PathBuf {
    if let Ok(xdg) = std::env::var("XDG_CONFIG_HOME") {
        if !xdg.is_empty() {
            return PathBuf::from(xdg).join("BambuStudio");
        }
    }
    #[cfg(windows)]
    {
        if let Ok(appdata) = std::env::var("APPDATA") {
            return PathBuf::from(appdata).join("BambuStudio");
        }
    }
    let home = std::env::var("HOME").unwrap_or_else(|_| ".".into());
    PathBuf::from(home).join(".config/BambuStudio")
}

pub fn lan_codes_path(dir: impl AsRef<Path>) -> PathBuf {
    dir.as_ref().join(LAN_CODES_FILE)
}

pub fn load_lan_codes(dir: impl AsRef<Path>) -> BTreeMap<String, String> {
    let text = std::fs::read_to_string(lan_codes_path(dir)).unwrap_or_default();
    serde_json::from_str(&text).unwrap_or_default()
}

pub fn save_lan_codes(
    dir: impl AsRef<Path>,
    codes: &BTreeMap<String, String>,
) -> Result<PathBuf, CredentialError> {
    let dir = dir.as_ref();
    std::fs::create_dir_all(dir)?;
    let path = lan_codes_path(dir);
    std::fs::write(&path, serde_json::to_vec_pretty(codes).unwrap_or_default())?;
    Ok(path)
}

/// Parse `BambuStudio.conf` JSON (LAN maps + region + default serial).
pub fn parse_studio_conf(
    text: &str,
) -> Result<(Vec<StudioPrinter>, String, String), CredentialError> {
    let v: Value = serde_json::from_str(text)
        .map_err(|err| CredentialError::Message(format!("BambuStudio.conf: {err}")))?;
    Ok(parse_studio_conf_value(&v))
}

fn parse_studio_conf_value(v: &Value) -> (Vec<StudioPrinter>, String, String) {
    let mut codes = BTreeMap::new();
    merge_code_map(&mut codes, v.get("access_code"));
    merge_code_map(&mut codes, v.get("user_access_code"));
    let printers: Vec<StudioPrinter> = codes
        .into_iter()
        .map(|(serial, access_code)| StudioPrinter {
            serial,
            access_code,
        })
        .collect();
    let mut dev_ids = Vec::new();
    collect_dev_ids(v, &mut dev_ids);
    let default_serial = v
        .pointer("/sync_extruder/dev_id")
        .and_then(Value::as_str)
        .map(str::to_string)
        .or_else(|| printers.first().map(|p| p.serial.clone()))
        .or_else(|| dev_ids.into_iter().next())
        .unwrap_or_default();
    let region = region_from_conf(v);
    (printers, default_serial, region)
}

fn merge_code_map(into: &mut BTreeMap<String, String>, v: Option<&Value>) {
    let Some(Value::Object(map)) = v else {
        return;
    };
    for (serial, code) in map {
        let Some(code) = code.as_str() else {
            continue;
        };
        if serial.is_empty() || code.is_empty() {
            continue;
        }
        into.insert(serial.clone(), code.to_string());
    }
}

fn collect_dev_ids(v: &Value, out: &mut Vec<String>) {
    match v {
        Value::Object(map) => {
            if let Some(id) = map.get("dev_id").and_then(Value::as_str) {
                if !id.is_empty() && !out.iter().any(|s| s == id) {
                    out.push(id.to_string());
                }
            }
            for child in map.values() {
                collect_dev_ids(child, out);
            }
        }
        Value::Array(items) => {
            for child in items {
                collect_dev_ids(child, out);
            }
        }
        _ => {}
    }
}

fn region_from_conf(v: &Value) -> String {
    if has_country_cn(v) {
        return "cn".into();
    }
    match v.get("iot_environment") {
        Some(Value::String(s)) if s == "1" || s.eq_ignore_ascii_case("cn") => "cn".into(),
        Some(Value::Number(n)) if n.as_i64() == Some(1) => "cn".into(),
        _ => "us".into(),
    }
}

fn has_country_cn(v: &Value) -> bool {
    match v {
        Value::Object(map) => {
            if let Some(c) = map.get("country").and_then(Value::as_str) {
                if c.eq_ignore_ascii_case("CN") {
                    return true;
                }
            }
            if let Some(c) = map.get("country_code").and_then(Value::as_str) {
                if c.eq_ignore_ascii_case("CN") {
                    return true;
                }
            }
            map.values().any(has_country_cn)
        }
        Value::Array(items) => items.iter().any(has_country_cn),
        _ => false,
    }
}

pub fn studio_user_id(studio_dir: impl AsRef<Path>) -> String {
    let user_dir = studio_dir.as_ref().join("user");
    let Ok(entries) = std::fs::read_dir(user_dir) else {
        return String::new();
    };
    let mut ids: Vec<String> = entries
        .flatten()
        .filter(|e| e.path().is_dir())
        .filter_map(|e| e.file_name().to_str().map(str::to_string))
        .filter(|name| !name.is_empty() && name.chars().all(|c| c.is_ascii_digit()))
        .collect();
    ids.sort();
    ids.pop().unwrap_or_default()
}

#[derive(Debug, Clone, Default)]
struct EngineTokens {
    access_token: String,
    refresh_token: String,
    country_code: String,
}

fn scan_engine_tokens() -> EngineTokens {
    let mut found = EngineTokens::default();
    let mut dirs = candidate_import_dirs();
    dirs.push(default_studio_data_dir());
    for dir in dirs {
        if !dir.is_dir() {
            continue;
        }
        scan_engine_dir(&dir, &mut found);
        if !found.access_token.is_empty() && !found.refresh_token.is_empty() {
            break;
        }
    }
    found
}

fn scan_engine_dir(dir: &Path, found: &mut EngineTokens) {
    let Ok(entries) = std::fs::read_dir(dir) else {
        return;
    };
    for entry in entries.flatten() {
        let path = entry.path();
        if path.is_dir() {
            continue;
        }
        let name = path
            .file_name()
            .and_then(|n| n.to_str())
            .unwrap_or("")
            .to_ascii_lowercase();
        let interesting = name.contains("network")
            || name.contains("engine")
            || name.ends_with(".conf")
            || name.ends_with(".json");
        if !interesting {
            continue;
        }
        let Ok(text) = std::fs::read_to_string(&path) else {
            continue;
        };
        merge_engine_text(&text, found);
    }
}

fn merge_engine_text(text: &str, found: &mut EngineTokens) {
    if let Ok(v) = serde_json::from_str::<Value>(text) {
        merge_engine_value(&v, found);
        return;
    }
    take_quoted(text, "accessToken", &mut found.access_token);
    take_quoted(text, "access_token", &mut found.access_token);
    take_quoted(text, "refreshToken", &mut found.refresh_token);
    take_quoted(text, "refresh_token", &mut found.refresh_token);
    take_quoted(text, "country_code", &mut found.country_code);
}

fn merge_engine_value(v: &Value, found: &mut EngineTokens) {
    if found.access_token.is_empty() {
        if let Some(s) = first_string(v, &["accessToken", "access_token"]) {
            found.access_token = s;
        }
    }
    if found.refresh_token.is_empty() {
        if let Some(s) = first_string(v, &["refreshToken", "refresh_token"]) {
            found.refresh_token = s;
        }
    }
    if found.country_code.is_empty() {
        if let Some(s) = first_string(v, &["country_code", "countryCode"]) {
            found.country_code = s;
        }
    }
    match v {
        Value::Object(map) => {
            for child in map.values() {
                merge_engine_value(child, found);
            }
        }
        Value::Array(items) => {
            for child in items {
                merge_engine_value(child, found);
            }
        }
        _ => {}
    }
}

fn first_string(v: &Value, keys: &[&str]) -> Option<String> {
    let Value::Object(map) = v else {
        return None;
    };
    for key in keys {
        if let Some(s) = map.get(*key).and_then(Value::as_str) {
            let s = s.trim();
            if !s.is_empty() {
                return Some(s.to_string());
            }
        }
    }
    None
}

fn take_quoted(text: &str, key: &str, dest: &mut String) {
    if !dest.is_empty() {
        return;
    }
    let needle = format!("\"{key}\"");
    let Some(idx) = text.find(&needle) else {
        return;
    };
    let rest = &text[idx + needle.len()..];
    let Some(colon) = rest.find(':') else {
        return;
    };
    let rest = rest[colon + 1..].trim_start();
    let Some(rest) = rest.strip_prefix('"') else {
        return;
    };
    let mut out = String::new();
    let mut chars = rest.chars();
    while let Some(ch) = chars.next() {
        if ch == '\\' {
            if let Some(next) = chars.next() {
                out.push(next);
            }
            continue;
        }
        if ch == '"' {
            break;
        }
        out.push(ch);
    }
    if !out.is_empty() {
        *dest = out;
    }
}

/// Copy Studio LAN maps + tokens, extract PEMs into the rewrite config dir.
///
/// Hard error if `slicer_cert.pem` + `slicer_key.pem` are still missing.
pub fn import_studio(
    studio_dir: Option<&Path>,
    out_dir: Option<&Path>,
) -> Result<StudioImport, CredentialError> {
    let studio = studio_dir
        .map(Path::to_path_buf)
        .unwrap_or_else(default_studio_data_dir);
    let dest = out_dir
        .map(Path::to_path_buf)
        .unwrap_or_else(default_config_dir);
    std::fs::create_dir_all(&dest)?;

    let mut extract = ExtractReport {
        credentials: crate::credentials::import_from_known_locations()?,
        ..ExtractReport::default()
    };
    merge_studio_plugins(&studio, &mut extract);
    if !extract.credentials.has_cert_and_key() {
        let scanned = extract_to_config_dir(None, Some(&dest))?;
        if extract.credentials.cert_pem.is_none() {
            extract.credentials.cert_pem = scanned.credentials.cert_pem;
        }
        if extract.credentials.key_pem.is_none() {
            extract.credentials.key_pem = scanned.credentials.key_pem;
        }
        if extract.credentials.crl_pem.is_none() {
            extract.credentials.crl_pem = scanned.credentials.crl_pem;
        }
        extract.notes.extend(scanned.notes);
        if extract.plugin.is_none() {
            extract.plugin = scanned.plugin;
        }
    }
    if extract.credentials.cert_pem.is_some()
        || extract.credentials.key_pem.is_some()
        || extract.credentials.crl_pem.is_some()
    {
        write_to_dir(&dest, &extract.credentials)?;
        extract
            .notes
            .push(format!("wrote credentials under {}", dest.display()));
    }

    let mut imported = StudioImport {
        extract,
        ..StudioImport::default()
    };

    let conf_path = studio.join("BambuStudio.conf");
    if let Ok(text) = std::fs::read_to_string(&conf_path) {
        let (printers, default_serial, region) = parse_studio_conf(&text)?;
        imported.printers = printers;
        imported.default_serial = default_serial;
        imported.region = region;
        imported.notes.push(format!("read {}", conf_path.display()));
    } else {
        imported
            .notes
            .push(format!("no BambuStudio.conf at {}", conf_path.display()));
    }

    imported.user_id = studio_user_id(&studio);
    if imported.user_id.is_empty() {
        imported
            .notes
            .push("no numeric user/<id>/ directory".into());
    }

    let mut tokens = EngineTokens::default();
    scan_engine_dir(&studio, &mut tokens);
    if tokens.access_token.is_empty() || tokens.refresh_token.is_empty() {
        let extra = scan_engine_tokens();
        if tokens.access_token.is_empty() {
            tokens.access_token = extra.access_token;
        }
        if tokens.refresh_token.is_empty() {
            tokens.refresh_token = extra.refresh_token;
        }
        if tokens.country_code.is_empty() {
            tokens.country_code = extra.country_code;
        }
    }
    if imported.region.is_empty() {
        imported.region = if tokens.country_code.eq_ignore_ascii_case("CN") {
            "cn".into()
        } else {
            "us".into()
        };
    } else if tokens.country_code.eq_ignore_ascii_case("CN") {
        imported.region = "cn".into();
    }
    imported.has_token = !tokens.access_token.is_empty();
    imported.has_refresh = !tokens.refresh_token.is_empty();

    let mut codes = BTreeMap::new();
    for printer in &imported.printers {
        codes.insert(printer.serial.clone(), printer.access_code.clone());
    }
    if !codes.is_empty() {
        save_lan_codes(&dest, &codes)?;
    }

    let session = CloudSession {
        region: imported.region.clone(),
        user_id: imported.user_id.clone(),
        access_token: tokens.access_token,
        refresh_token: tokens.refresh_token,
        serial: imported.default_serial.clone(),
    };
    save_cloud_session(&dest, &session)?;

    if !imported.extract.credentials.has_cert_and_key() {
        return Err(CredentialError::Message(
            "slicer_cert.pem and slicer_key.pem still missing after scanning plugins and config dirs"
                .into(),
        ));
    }
    Ok(imported)
}

fn merge_studio_plugins(studio: &Path, extract: &mut ExtractReport) {
    for dir in [studio.join("plugins"), studio.join("Plugins")] {
        let Ok(entries) = std::fs::read_dir(&dir) else {
            continue;
        };
        for entry in entries.flatten() {
            let path = entry.path();
            let name = path
                .file_name()
                .and_then(|n| n.to_str())
                .unwrap_or("")
                .to_ascii_lowercase();
            if !(name.contains("bambu_networking") || name.contains("bambunetwork")) {
                continue;
            }
            extract.notes.push(format!("scanning {}", path.display()));
            if let Ok(bytes) = std::fs::read(&path) {
                let found = crate::extract::extract_pems_from_bytes(&bytes);
                if extract.credentials.cert_pem.is_none() {
                    extract.credentials.cert_pem = found.cert_pem;
                }
                if extract.credentials.key_pem.is_none() {
                    extract.credentials.key_pem = found.key_pem;
                }
                if extract.credentials.crl_pem.is_none() {
                    extract.credentials.crl_pem = found.crl_pem;
                }
                if extract.plugin.is_none() {
                    extract.plugin = Some(path);
                }
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_studio_conf_fixture() {
        let text = include_str!("../tests/fixtures/BambuStudio.conf");
        let (printers, serial, region) = parse_studio_conf(text).unwrap();
        assert_eq!(region, "us");
        assert_eq!(serial, "01P00AFAKE00001");
        assert_eq!(printers.len(), 2);
        assert!(printers.iter().any(|p| p.serial == "01P00AFAKE00002"));
        assert!(printers.iter().all(
            |p| !p.access_code.is_empty() && p.access_code.chars().all(|c| c.is_ascii_digit())
        ));
    }

    #[test]
    fn cn_country_overrides_iot_environment() {
        let v = serde_json::json!({
            "iot_environment": "3",
            "country": "CN",
            "access_code": {"01X": "12345678"}
        });
        let (printers, serial, region) = parse_studio_conf_value(&v);
        assert_eq!(region, "cn");
        assert_eq!(serial, "01X");
        assert_eq!(printers[0].access_code, "12345678");
    }

    #[test]
    fn import_writes_pems_and_lan_map() {
        let cert = include_str!("../tests/fixtures/test_slicer_cert.pem");
        let key = include_str!("../tests/fixtures/test_slicer_key.pem");
        let mut blob = Vec::from(&b"hdr\0"[..]);
        blob.extend(cert.as_bytes());
        blob.extend(b"\n");
        blob.extend(key.as_bytes());
        let tmp = std::env::temp_dir().join(format!(
            "bambu-studio-import-{}-{}",
            std::process::id(),
            line!()
        ));
        let _ = std::fs::remove_dir_all(&tmp);
        let studio = tmp.join("studio");
        let out = tmp.join("cfg");
        let plugins = studio.join("plugins");
        std::fs::create_dir_all(plugins.join("keep")).unwrap();
        std::fs::create_dir_all(studio.join("user/424242")).unwrap();
        std::fs::write(
            studio.join("BambuStudio.conf"),
            include_str!("../tests/fixtures/BambuStudio.conf"),
        )
        .unwrap();
        std::fs::write(plugins.join("libbambu_networking.so"), blob).unwrap();
        std::fs::write(
            studio.join("BambuNetworkEngine.conf"),
            r#"{"accessToken":"tok_test_not_real","refreshToken":"ref_test_not_real"}"#,
        )
        .unwrap();

        let imported = import_studio(Some(&studio), Some(&out)).unwrap();
        assert_eq!(imported.user_id, "424242");
        assert_eq!(imported.region, "us");
        assert!(imported.has_token);
        assert!(out.join("slicer_cert.pem").is_file());
        assert!(out.join("slicer_key.pem").is_file());
        let codes = load_lan_codes(&out);
        assert_eq!(
            codes.get("01P00AFAKE00001").map(String::as_str),
            Some("87654321")
        );
        let session = crate::cloud::load_cloud_session(&out).unwrap();
        assert_eq!(session.user_id, "424242");
        assert_eq!(session.access_token, "tok_test_not_real");
        let _ = std::fs::remove_dir_all(&tmp);
    }
}
