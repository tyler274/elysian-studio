//! Pull Option B PEMs out of a local stock `libbambu_networking` (or Bambu Connect) blob.
//!
//! ClusterM does not ship Bambu's signing material and neither do we. The stock
//! plugin you already downloaded is scanned for PEM armor (plaintext, UTF-16, or
//! single-byte XOR). Results are written next to the rewrite config, never into
//! the source tree.

use std::path::{Path, PathBuf};
use std::time::Duration;

use rsa::traits::PublicKeyParts;

use crate::credentials::{default_config_dir, import_from_known_locations, SlicerCredentials};
use crate::signing::{load_private_key, public_key_from_cert_pem};

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
                if let Some(found) = crate::extract_bootstrap::harvest_bootstrap(&bytes, &[]) {
                    merge_creds(&mut report.credentials, found);
                }
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
        || report.credentials.client_auth_secret.is_some()
        || report.credentials.server_wrap_pem.is_some()
    {
        crate::credentials::write_to_dir(&dest, &report.credentials)?;
        report
            .notes
            .push(format!("wrote credentials under {}", dest.display()));
    } else {
        report.notes.push(
            "no PEMs in the on-disk plugin (recent builds obfuscate past a static scan); live Studio extract can harvest decrypted PEMs from a sandboxed official process"
                .into(),
        );
    }
    Ok(report)
}

#[derive(Debug, Clone)]
pub struct ExtractKeysOpts {
    pub plugin: Option<PathBuf>,
    pub out_dir: Option<PathBuf>,
    pub live: bool,
    pub unpack: bool,
    pub dump_elf: Option<PathBuf>,
    /// Existing helper/Studio dump to scan (skips a new VMProtect unpack when set).
    pub from_dump: Option<PathBuf>,
    pub timeout: Duration,
}

impl Default for ExtractKeysOpts {
    fn default() -> Self {
        Self {
            plugin: None,
            out_dir: None,
            live: true,
            unpack: true,
            dump_elf: None,
            from_dump: None,
            timeout: Duration::from_secs(90),
        }
    }
}

/// Static plugin scan, optional dump / self-unpack, cloud cert GET, then Studio harvest.
pub fn extract_keys(
    opts: ExtractKeysOpts,
) -> Result<ExtractReport, crate::credentials::CredentialError> {
    let mut report = extract_to_config_dir(opts.plugin.as_deref(), opts.out_dir.as_deref())?;
    if report.credentials.has_cert_and_key() && report.credentials.has_bootstrap() {
        return Ok(report);
    }
    if let Some(path) = &opts.from_dump {
        match std::fs::read(path) {
            Ok(bytes) => {
                report
                    .notes
                    .push(format!("scanning dump {}", path.display()));
                apply_appcert_dump(&mut report, &bytes, &[], "dump");
                write_extracted(&mut report, opts.out_dir.as_deref())?;
                if report.credentials.has_cert_and_key() && report.credentials.has_bootstrap() {
                    return Ok(report);
                }
            }
            Err(err) => report
                .notes
                .push(format!("could not read dump {}: {err}", path.display())),
        }
    }
    if !opts.unpack {
        if let Some(path) = opts
            .plugin
            .clone()
            .or_else(|| report.plugin.clone())
            .or_else(find_stock_plugin)
        {
            report.notes.extend(crate::extract_elf::map_notes(&path));
        }
    }
    if opts.unpack && !report.credentials.has_bootstrap() {
        crate::extract_unpack::extract_unpack(
            &mut report,
            opts.plugin.as_deref(),
            opts.out_dir.as_deref(),
            opts.dump_elf.as_deref(),
            opts.timeout,
        )?;
        if report.credentials.has_cert_and_key() && report.credentials.has_bootstrap() {
            return Ok(report);
        }
    }
    if !report.credentials.can_sign() {
        try_cloud_appcert(&mut report, opts.out_dir.as_deref())?;
        if report.credentials.has_cert_and_key() && report.credentials.has_bootstrap() {
            return Ok(report);
        }
    }
    if !opts.live || report.credentials.can_sign() {
        return Ok(report);
    }
    report.notes.push(
        "starting sandboxed official Studio to harvest decrypted PEMs (this process does not dlopen the plugin)"
            .into(),
    );
    crate::extract_live::extract_live(
        &mut report,
        opts.plugin.as_deref(),
        opts.out_dir.as_deref(),
        opts.timeout,
    )?;
    Ok(report)
}

pub(crate) fn apply_appcert_dump(
    report: &mut ExtractReport,
    bytes: &[u8],
    rand: &[u8],
    prefix: &str,
) {
    if let Some(found) = crate::extract_appcert::harvest_appcert(bytes) {
        merge_creds(&mut report.credentials, found);
        report.notes.push(format!(
            "{prefix}: unwrapped get_app_cert JSON (custom AES-CTR, ClusterM fetch_slicer_credentials.py)"
        ));
    }
    if let Some(found) = crate::extract_bootstrap::harvest_bootstrap(bytes, rand) {
        let had_secret = found.client_auth_secret.is_some();
        let had_wrap = found.server_wrap_pem.is_some();
        merge_creds(&mut report.credentials, found);
        if had_secret {
            report
                .notes
                .push(format!("{prefix}: recovered client_auth_secret"));
        }
        if had_wrap {
            report
                .notes
                .push(format!("{prefix}: recovered server_wrap_key"));
        }
    }
    merge_creds(
        &mut report.credentials,
        crate::extract_elf::scan_image(bytes),
    );
}

fn write_extracted(
    report: &mut ExtractReport,
    out_dir: Option<&Path>,
) -> Result<(), crate::credentials::CredentialError> {
    if report.credentials.cert_pem.is_none()
        && report.credentials.key_pem.is_none()
        && report.credentials.crl_pem.is_none()
        && report.credentials.client_auth_secret.is_none()
        && report.credentials.server_wrap_pem.is_none()
    {
        return Ok(());
    }
    let dest = out_dir
        .map(Path::to_path_buf)
        .unwrap_or_else(default_config_dir);
    crate::credentials::write_to_dir(&dest, &report.credentials)?;
    report
        .notes
        .push(format!("wrote credentials under {}", dest.display()));
    Ok(())
}

fn try_cloud_appcert(
    report: &mut ExtractReport,
    out_dir: Option<&Path>,
) -> Result<(), crate::credentials::CredentialError> {
    let dest = out_dir
        .map(Path::to_path_buf)
        .unwrap_or_else(default_config_dir);
    let mut dirs = vec![dest.clone()];
    if let Ok(home) = std::env::var("HOME") {
        dirs.push(PathBuf::from(&home).join(".config/BambuStudio"));
        dirs.push(PathBuf::from(&home).join(".config/open-bamboo-networking"));
    }
    let secret_wrap = report
        .credentials
        .client_auth_secret
        .as_ref()
        .zip(report.credentials.server_wrap_pem.as_ref())
        .map(|(s, w)| (s.clone(), w.as_bytes().to_vec()))
        .or_else(|| crate::extract_appcert::try_bootstrap_secret_files(&dirs));
    let Some((secret, wrap)) = secret_wrap else {
        return Ok(());
    };
    let region = std::fs::read_to_string(dest.join("cloud_region")).unwrap_or_default();
    let host = crate::cloud_api::api_host(region.trim());
    report.notes.push(format!(
        "cloud: GET applications/{{enc_secret}}/cert using bootstrap files ({} byte secret)",
        secret.len()
    ));
    match crate::extract_appcert::fetch_appcert_from_bootstrap(&secret, &wrap, host) {
        Ok(found) => {
            merge_creds(&mut report.credentials, found);
            write_extracted(report, out_dir)?;
            report
                .notes
                .push("cloud: fetched app cert/key (ClusterM envelope)".into());
        }
        Err(err) => report.notes.push(format!("cloud: {err}")),
    }
    Ok(())
}

pub(crate) fn merge_creds(into: &mut SlicerCredentials, from: SlicerCredentials) {
    let key = into.key_pem.as_deref().or(from.key_pem.as_deref());
    into.cert_pem = pick_cert(into.cert_pem.take(), from.cert_pem, key);
    if into.key_pem.is_none() {
        into.key_pem = from.key_pem;
    }
    if into.crl_pem.is_none() {
        into.crl_pem = from.crl_pem;
    }
    if into.client_auth_secret.is_none() {
        into.client_auth_secret = from.client_auth_secret;
    }
    if into.server_wrap_pem.is_none() {
        into.server_wrap_pem = from.server_wrap_pem;
    }
}

fn pick_cert(a: Option<String>, b: Option<String>, key: Option<&str>) -> Option<String> {
    let score = |pem: &str| -> i32 {
        let mut s = pem.matches("-----BEGIN CERTIFICATE-----").count() as i32;
        if let Some(k) = key {
            if let (Ok(privk), Ok(pubk)) = (load_private_key(k), public_key_from_cert_pem(pem)) {
                if privk.n() == pubk.n() {
                    s += 100;
                }
            }
        }
        s
    };
    match (a, b) {
        (None, x) | (x, None) => x,
        (Some(a), Some(b)) => {
            if score(&b) > score(&a) {
                Some(b)
            } else {
                Some(a)
            }
        }
    }
}

pub fn extract_pems_from_bytes(data: &[u8]) -> SlicerCredentials {
    extract_pems_from_bytes_ex(data, true)
}

/// ASCII / UTF-16 PEM scan without a 255-key XOR pass (too slow on 32MB dumps).
pub(crate) fn extract_pems_plain(data: &[u8]) -> SlicerCredentials {
    extract_pems_from_bytes_ex(data, false)
}

fn extract_pems_from_bytes_ex(data: &[u8], xor: bool) -> SlicerCredentials {
    let mut creds = SlicerCredentials::default();
    collect_pems(&mut creds, &decode_ascii(data));
    if data.len() <= 4 * 1024 * 1024 && (!creds.can_sign() || creds.cert_pem.is_none()) {
        collect_pems(&mut creds, &decode_utf16le(data));
    }
    if xor && data.len() <= 4 * 1024 * 1024 && (!creds.can_sign() || creds.cert_pem.is_none()) {
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
        scan_plugin_dir(&mut out, &home.join(".config/OrcaSlicer/plugins"));
        scan_plugin_dir(
            &mut out,
            &home.join("Library/Application Support/BambuStudio/plugins"),
        );
        scan_plugin_dir(
            &mut out,
            &home.join("Library/Application Support/OrcaSlicer/plugins"),
        );
        push_plugin(
            &mut out,
            home.join("Library/Application Support/BambuStudio/plugins/libbambu_networking.dylib"),
        );
    }
    if let Ok(xdg) = std::env::var("XDG_CONFIG_HOME") {
        let xdg = PathBuf::from(xdg);
        scan_plugin_dir(&mut out, &xdg.join("BambuStudio/plugins"));
        scan_plugin_dir(&mut out, &xdg.join("OrcaSlicer/plugins"));
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
            &PathBuf::from(&appdata).join("BambuStudio/plugins"),
        );
        scan_plugin_dir(&mut out, &PathBuf::from(appdata).join("OrcaSlicer/plugins"));
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
    scan_plugin_dir_depth(out, dir, 0);
}

fn scan_plugin_dir_depth(out: &mut Vec<PathBuf>, dir: &Path, depth: u8) {
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
        if path.is_dir() && depth == 0 && name == "backup" {
            scan_plugin_dir_depth(out, &path, depth + 1);
            continue;
        }
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

    #[test]
    fn scan_plugin_dir_picks_versioned_orca_name() {
        let tmp = std::env::temp_dir().join(format!(
            "bambu-orca-plugin-{}-{}",
            std::process::id(),
            line!()
        ));
        let _ = std::fs::remove_dir_all(&tmp);
        let plugins = tmp.join("plugins");
        std::fs::create_dir_all(plugins.join("backup")).unwrap();
        std::fs::write(
            plugins.join("libbambu_networking_02.03.00.62.so"),
            b"not-a-pem",
        )
        .unwrap();
        std::fs::write(
            plugins.join("backup").join("libbambu_networking.so"),
            b"also-not",
        )
        .unwrap();
        let mut found = Vec::new();
        scan_plugin_dir(&mut found, &plugins);
        assert!(
            found.iter().any(|p| p
                .file_name()
                .and_then(|n| n.to_str())
                .is_some_and(|n| n.contains("02.03.00.62"))),
            "{found:?}"
        );
        assert!(
            found.iter().any(|p| p
                .parent()
                .and_then(|d| d.file_name())
                .and_then(|n| n.to_str())
                == Some("backup")),
            "{found:?}"
        );
        let _ = std::fs::remove_dir_all(&tmp);
    }
}
