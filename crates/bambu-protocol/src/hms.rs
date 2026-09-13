//! Public HMS catalog (`https://e.bambulab.com/query.php`) — no plugin, no PEMs.

use std::io::{Read, Write};
use std::net::TcpStream;
use std::path::{Path, PathBuf};
use std::time::Duration;

use bambu_device::HmsCode;
use rustls::pki_types::ServerName;
use rustls::{ClientConnection, Stream};
use serde_json::Value;
use thiserror::Error;

use crate::tls::{install_ring_provider, lan_client_config, TlsError};

pub const HMS_HOST: &str = "e.bambulab.com";

#[derive(Debug, Error)]
pub enum HmsError {
    #[error("hms: {0}")]
    Message(String),
    #[error(transparent)]
    Tls(#[from] TlsError),
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
    let body = https_get(HMS_HOST, &path)?;
    let value: Value = serde_json::from_str(&body)?;
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

fn https_get(host: &str, path: &str) -> Result<String, HmsError> {
    install_ring_provider();
    let config = lan_client_config()?;
    let name = ServerName::try_from(host.to_string())
        .map_err(|err| HmsError::Message(format!("server name {host}: {err}")))?;
    let mut tcp = TcpStream::connect((host, 443))?;
    tcp.set_read_timeout(Some(Duration::from_secs(20)))?;
    tcp.set_write_timeout(Some(Duration::from_secs(20)))?;
    let mut conn =
        ClientConnection::new(config, name).map_err(|err| HmsError::Message(err.to_string()))?;
    let mut tls = Stream::new(&mut conn, &mut tcp);
    let req = format!(
        "GET {path} HTTP/1.1\r\nHost: {host}\r\nAccept: application/json\r\nConnection: close\r\n\r\n"
    );
    tls.write_all(req.as_bytes())?;
    tls.flush()?;
    let mut raw = Vec::new();
    tls.read_to_end(&mut raw)?;
    let text = String::from_utf8_lossy(&raw);
    split_http_body(&text).ok_or_else(|| HmsError::Message("HTTP response had no body".into()))
}

fn split_http_body(response: &str) -> Option<String> {
    let (headers, body) = response.split_once("\r\n\r\n")?;
    let status = headers.lines().next().unwrap_or("");
    if !status.contains(" 200 ") && !status.ends_with(" 200") {
        return None;
    }
    let chunked = headers.lines().any(|l| {
        l.to_ascii_lowercase().starts_with("transfer-encoding:")
            && l.to_ascii_lowercase().contains("chunked")
    });
    if chunked {
        decode_chunked(body)
    } else {
        Some(body.to_string())
    }
}

fn decode_chunked(body: &str) -> Option<String> {
    let mut rest = body;
    let mut out = String::new();
    loop {
        let (size_line, after) = rest.split_once("\r\n")?;
        let size =
            usize::from_str_radix(size_line.trim().split(';').next().unwrap_or(""), 16).ok()?;
        if size == 0 {
            break;
        }
        if after.len() < size {
            return None;
        }
        out.push_str(&after[..size]);
        rest = after
            .get(size..)?
            .strip_prefix("\r\n")
            .unwrap_or(&after[size..]);
    }
    Some(out)
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
