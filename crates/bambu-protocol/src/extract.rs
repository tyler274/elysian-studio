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
    if report.credentials.can_sign() {
        report.notes.push(
            "loaded existing slicer_*.pem from a config directory (Open Bamboo Networking or Studio)"
                .into(),
        );
    }

    let plugin_path = plugin.map(Path::to_path_buf).or_else(find_stock_plugin);
    if let Some(path) = plugin_path {
        report.notes.push(format!("scanning {}", path.display()));
        match std::fs::read(&path) {
            Ok(bytes) => {
                let found = extract_pems_from_bytes(&bytes);
                merge_creds(&mut report.credentials, found);
                report.plugin = Some(path);
            }
            Err(err) => report.notes.push(format!("could not read plugin: {err}")),
        }
    } else {
        report.notes.push(
            "no stock plugin found; pass --plugin /path/to/libbambu_networking.so or set BAMBU_NETWORKING_PLUGIN"
                .into(),
        );
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
    if let Ok(p) = std::env::var("BAMBU_NETWORKING_PLUGIN") {
        let path = PathBuf::from(p);
        if path.is_file() {
            return Some(path);
        }
    }
    let mut candidates = Vec::new();
    if let Ok(home) = std::env::var("HOME") {
        let home = PathBuf::from(home);
        candidates.extend([
            home.join(".local/share/BambuStudio/plugins/libbambu_networking.so"),
            home.join(".BambuStudio/plugins/libbambu_networking.so"),
            home.join(".config/BambuStudio/plugins/libbambu_networking.so"),
            home.join("Library/Application Support/BambuStudio/plugins/libbambu_networking.dylib"),
        ]);
    }
    if let Ok(studio) = std::env::var("BAMBU_STUDIO") {
        let bin = PathBuf::from(studio);
        if let Some(dir) = bin.parent() {
            candidates.push(dir.join("libbambu_networking.so"));
            candidates.push(dir.join("../lib/libbambu_networking.so"));
            candidates.push(dir.join("../plugins/libbambu_networking.so"));
        }
    }
    #[cfg(windows)]
    if let Ok(appdata) = std::env::var("APPDATA") {
        candidates.push(PathBuf::from(appdata).join("BambuStudio/plugins/bambu_networking.dll"));
    }
    candidates.into_iter().find(|p| p.is_file())
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
}
