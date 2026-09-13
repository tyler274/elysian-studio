//! Pull Option B PEMs out of a local stock `libbambu_networking` (or Bambu Connect) blob.
//!
//! ClusterM does not ship Bambu's signing material and neither do we. The stock
//! plugin you already downloaded is scanned for PEM armor (plaintext, UTF-16, or
//! single-byte XOR). Results are written next to the rewrite config, never into
//! the source tree.

use std::path::{Path, PathBuf};

use crate::credentials::{default_config_dir, import_from_known_locations, SlicerCredentials};

#[derive(Debug, Clone, Default)]
pub struct ExtractReport {
    pub plugin: Option<PathBuf>,
    pub credentials: SlicerCredentials,
    pub notes: Vec<String>,
}

pub fn extract_to_config_dir(
    plugin: Option<&Path>,
    out_dir: Option<&Path>,
) -> Result<ExtractReport, crate::credentials::CredentialError> {
    let mut report = ExtractReport::default();
    let imported = import_from_known_locations()?;
    report.credentials = imported;
    if report.credentials.has_cert_and_key() {
        report.notes.push(
            "loaded existing slicer_*.pem from a config directory (Open Bamboo Networking or Studio)"
                .into(),
        );
    }

    let plugins = if let Some(path) = plugin {
        vec![path.to_path_buf()]
    } else {
        find_all_stock_plugins()
    };
    if plugins.is_empty() {
        report.notes.push(
            "no stock plugin found; pass --plugin /path/to/libbambu_networking.so or set BAMBU_NETWORKING_PLUGIN"
                .into(),
        );
    }
    for path in plugins {
        report.notes.push(format!("scanning {}", path.display()));
        match std::fs::read(&path) {
            Ok(bytes) => {
                let found = extract_pems_from_bytes(&bytes);
                merge_creds(&mut report.credentials, found);
                if report.plugin.is_none() {
                    report.plugin = Some(path);
                }
            }
            Err(err) => report.notes.push(format!("could not read plugin: {err}")),
        }
        if report.credentials.has_cert_and_key() && report.credentials.crl_pem.is_some() {
            break;
        }
    }

    let dest = out_dir
        .map(Path::to_path_buf)
        .unwrap_or_else(default_config_dir);
    if report.credentials.cert_pem.is_some()
        || report.credentials.key_pem.is_some()
        || report.credentials.crl_pem.is_some()
    {
        crate::credentials::write_to_dir(&dest, &report.credentials)?;
        report
            .notes
            .push(format!("wrote credentials under {}", dest.display()));
    } else {
        report.notes.push(
            "no PEMs found. Recent plugins obfuscate keys past a simple scan; import slicer_cert.pem / slicer_key.pem / slicer_crl.pem extracted by a local tool such as BambuSlicerKeySaver into the config dir."
                .into(),
        );
    }
    Ok(report)
}

fn merge_creds(into: &mut SlicerCredentials, from: SlicerCredentials) {
    if into.cert_pem.is_none() {
        into.cert_pem = from.cert_pem;
    }
    if into.key_pem.is_none() {
        into.key_pem = from.key_pem;
    }
    if into.crl_pem.is_none() {
        into.crl_pem = from.crl_pem;
    }
}

pub fn extract_pems_from_bytes(data: &[u8]) -> SlicerCredentials {
    let mut creds = SlicerCredentials::default();
    collect_pems(&mut creds, &decode_ascii(data));
    if !creds.can_sign() || creds.cert_pem.is_none() {
        collect_pems(&mut creds, &decode_utf16le(data));
    }
    if !creds.can_sign() || creds.cert_pem.is_none() {
        if let Some(decoded) = decode_xor_pem(data) {
            collect_pems(&mut creds, &decoded);
        }
    }
    creds
}

fn collect_pems(creds: &mut SlicerCredentials, text: &str) {
    for block in pem_blocks(text) {
        let upper = block.to_ascii_uppercase();
        if upper.contains("BEGIN CERTIFICATE") && creds.cert_pem.is_none() {
            creds.cert_pem = Some(block);
        } else if (upper.contains("BEGIN PRIVATE KEY") || upper.contains("BEGIN RSA PRIVATE KEY"))
            && creds.key_pem.is_none()
        {
            creds.key_pem = Some(block);
        } else if (upper.contains("BEGIN X509 CRL") || upper.contains("BEGIN CRL"))
            && creds.crl_pem.is_none()
        {
            creds.crl_pem = Some(block);
        }
    }
}

fn pem_blocks(text: &str) -> Vec<String> {
    let mut out = Vec::new();
    let bytes = text.as_bytes();
    let mut i = 0;
    while i < bytes.len() {
        let rest = &text[i..];
        let Some(begin) = rest.find("-----BEGIN ") else {
            break;
        };
        let start = i + begin;
        let after = &text[start..];
        let Some(end_rel) = after.find("-----END ") else {
            break;
        };
        let tail = &after[end_rel + "-----END ".len()..];
        let Some(nl) = tail.find("-----") else {
            break;
        };
        let end = start + end_rel + "-----END ".len() + nl + 5;
        if end <= text.len() {
            out.push(text[start..end].trim().to_string() + "\n");
        }
        i = end;
    }
    out
}

fn decode_ascii(data: &[u8]) -> String {
    String::from_utf8_lossy(data).into_owned()
}

fn decode_utf16le(data: &[u8]) -> String {
    let units: Vec<u16> = data
        .as_chunks::<2>()
        .0
        .iter()
        .map(|c| u16::from_le_bytes(*c))
        .collect();
    String::from_utf16_lossy(&units)
}

fn decode_xor_pem(data: &[u8]) -> Option<String> {
    let needle = b"-----BEGIN ";
    for key in 1u8..=255 {
        let xored: Vec<u8> = needle.iter().map(|b| b ^ key).collect();
        if find_bytes(data, &xored).is_some() {
            let decoded: String = data.iter().map(|b| (b ^ key) as char).collect();
            if decoded.contains("-----BEGIN ") {
                return Some(decoded);
            }
        }
    }
    None
}

fn find_bytes(hay: &[u8], needle: &[u8]) -> Option<usize> {
    hay.windows(needle.len()).position(|w| w == needle)
}

pub fn find_stock_plugin() -> Option<PathBuf> {
    find_all_stock_plugins().into_iter().next()
}

/// Local plugin blobs only — never `dlopen`.
pub fn find_all_stock_plugins() -> Vec<PathBuf> {
    let mut out = Vec::new();
    if let Ok(p) = std::env::var("BAMBU_NETWORKING_PLUGIN") {
        push_plugin(&mut out, PathBuf::from(p));
    }
    if let Ok(home) = std::env::var("HOME") {
        let home = PathBuf::from(home);
        scan_plugin_dir(&mut out, &home.join(".local/share/BambuStudio/plugins"));
        scan_plugin_dir(&mut out, &home.join(".BambuStudio/plugins"));
        scan_plugin_dir(&mut out, &home.join(".config/BambuStudio/plugins"));
        scan_plugin_dir(
            &mut out,
            &home.join("Library/Application Support/BambuStudio/plugins"),
        );
        push_plugin(
            &mut out,
            home.join("Library/Application Support/BambuStudio/plugins/libbambu_networking.dylib"),
        );
    }
    if let Ok(xdg) = std::env::var("XDG_CONFIG_HOME") {
        scan_plugin_dir(&mut out, &PathBuf::from(xdg).join("BambuStudio/plugins"));
    }
    if let Ok(studio) = std::env::var("BAMBU_STUDIO") {
        collect_neighbors(&mut out, Path::new(&studio));
    }
    if let Ok(path) = std::env::var("PATH") {
        for dir in std::env::split_paths(&path) {
            let bin = dir.join("bambu-studio");
            if bin.is_file() {
                collect_neighbors(&mut out, &bin);
            }
        }
    }
    #[cfg(windows)]
    if let Ok(appdata) = std::env::var("APPDATA") {
        scan_plugin_dir(
            &mut out,
            &PathBuf::from(appdata).join("BambuStudio/plugins"),
        );
    }
    out
}

fn collect_neighbors(out: &mut Vec<PathBuf>, studio_bin: &Path) {
    let Some(dir) = studio_bin.parent() else {
        return;
    };
    for rel in [
        "libbambu_networking.so",
        "bambu_networking.dll",
        "libbambu_networking.dylib",
        "../lib/libbambu_networking.so",
        "../plugins/libbambu_networking.so",
        "../lib/bambu-studio/plugins/libbambu_networking.so",
        "../lib64/bambu-studio/plugins/libbambu_networking.so",
    ] {
        push_plugin(out, dir.join(rel));
    }
    scan_plugin_dir(out, &dir.join("plugins"));
    if let Ok(canon) = dir.join("..").canonicalize() {
        scan_plugin_dir(out, &canon.join("plugins"));
        scan_plugin_dir(out, &canon.join("lib/bambu-studio/plugins"));
    }
}

fn scan_plugin_dir(out: &mut Vec<PathBuf>, dir: &Path) {
    let Ok(entries) = std::fs::read_dir(dir) else {
        return;
    };
    for entry in entries.flatten() {
        let path = entry.path();
        let name = path
            .file_name()
            .and_then(|n| n.to_str())
            .unwrap_or("")
            .to_ascii_lowercase();
        if name.contains("bambu_networking") || name.contains("bambunetwork") {
            push_plugin(out, path);
        }
    }
}

fn push_plugin(out: &mut Vec<PathBuf>, path: PathBuf) {
    let path = path.canonicalize().unwrap_or(path);
    if path.is_file() && !out.iter().any(|p| p == &path) {
        out.push(path);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn finds_pem_in_noise() {
        let cert = include_str!("../tests/fixtures/test_slicer_cert.pem");
        let key = include_str!("../tests/fixtures/test_slicer_key.pem");
        let mut blob = Vec::from(&b"junk\0\0"[..]);
        blob.extend(cert.as_bytes());
        blob.extend(b"\nmore junk\n");
        blob.extend(key.as_bytes());
        let creds = extract_pems_from_bytes(&blob);
        assert!(creds.cert_pem.unwrap().contains("BEGIN CERTIFICATE"));
        assert!(creds.key_pem.unwrap().contains("BEGIN PRIVATE KEY"));
    }

    #[test]
    fn finds_xor_obfuscated_pem() {
        let cert = include_str!("../tests/fixtures/test_slicer_cert.pem");
        let xored: Vec<u8> = cert.bytes().map(|b| b ^ 0x5A).collect();
        let creds = extract_pems_from_bytes(&xored);
        assert!(creds.cert_pem.unwrap().contains("BEGIN CERTIFICATE"));
    }

    #[test]
    fn extract_writes_pems_under_config_dir() {
        let cert = include_str!("../tests/fixtures/test_slicer_cert.pem");
        let key = include_str!("../tests/fixtures/test_slicer_key.pem");
        let mut blob = Vec::from(&b"noise\0"[..]);
        blob.extend(cert.as_bytes());
        blob.extend(b"\n");
        blob.extend(key.as_bytes());
        let tmp =
            std::env::temp_dir().join(format!("bambu-extract-{}-{}", std::process::id(), line!()));
        let _ = std::fs::remove_dir_all(&tmp);
        std::fs::create_dir_all(&tmp).unwrap();
        let plugin = tmp.join("libbambu_networking.so");
        std::fs::write(&plugin, blob).unwrap();
        let out = tmp.join("cfg");
        let report = extract_to_config_dir(Some(&plugin), Some(&out)).unwrap();
        assert!(report.credentials.has_cert_and_key());
        assert!(out.join("slicer_cert.pem").is_file());
        assert!(out.join("slicer_key.pem").is_file());
        let _ = std::fs::remove_dir_all(&tmp);
    }
}
