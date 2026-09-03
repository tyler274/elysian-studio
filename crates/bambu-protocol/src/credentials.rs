//! Slicer credential files used for [Option B](https://github.com/ClusterM/open-bamboo-networking#option-b-cloud-mode-without-developer-mode).
//!
//! This crate never ships Bambu's keys. Load `slicer_cert.pem`,
//! `slicer_key.pem`, and `slicer_crl.pem` that you extracted from a local
//! stock plugin (or imported from Open Bamboo Networking's config dir).

use std::path::{Path, PathBuf};

use thiserror::Error;

#[derive(Debug, Error)]
pub enum CredentialError {
    #[error("io: {0}")]
    Io(#[from] std::io::Error),
    #[error("{0}")]
    Message(String),
}

#[derive(Debug, Clone, Default)]
pub struct SlicerCredentials {
    pub cert_pem: Option<String>,
    pub key_pem: Option<String>,
    pub crl_pem: Option<String>,
}

impl SlicerCredentials {
    pub fn can_sign(&self) -> bool {
        self.key_pem.as_ref().is_some_and(|s| s.contains("BEGIN"))
    }

    pub fn can_install_app_cert(&self) -> bool {
        self.cert_pem
            .as_ref()
            .is_some_and(|s| s.contains("BEGIN CERTIFICATE"))
            && self.crl_pem.as_ref().is_some_and(|s| s.contains("BEGIN"))
    }

    pub fn status_lines(&self) -> Vec<String> {
        vec![
            format!(
                "slicer_cert.pem: {}",
                if self.cert_pem.is_some() {
                    "present"
                } else {
                    "missing"
                }
            ),
            format!(
                "slicer_key.pem: {}",
                if self.key_pem.is_some() {
                    "present"
                } else {
                    "missing"
                }
            ),
            format!(
                "slicer_crl.pem: {}",
                if self.crl_pem.is_some() {
                    "present"
                } else {
                    "missing"
                }
            ),
            format!(
                "Option B MQTT signing: {}",
                if self.can_sign() {
                    "ready"
                } else {
                    "needs slicer_key.pem"
                }
            ),
            format!(
                "app_cert_install: {}",
                if self.can_install_app_cert() {
                    "ready"
                } else {
                    "needs cert + CRL"
                }
            ),
        ]
    }
}

/// Config directory for extracted credentials (`$XDG_CONFIG_HOME/bambu-studio-rs`).
pub fn default_config_dir() -> PathBuf {
    if let Ok(xdg) = std::env::var("XDG_CONFIG_HOME") {
        if !xdg.is_empty() {
            return PathBuf::from(xdg).join("bambu-studio-rs");
        }
    }
    #[cfg(windows)]
    {
        if let Ok(appdata) = std::env::var("APPDATA") {
            return PathBuf::from(appdata).join("bambu-studio-rs");
        }
    }
    let home = std::env::var("HOME").unwrap_or_else(|_| ".".into());
    PathBuf::from(home).join(".config/bambu-studio-rs")
}

pub fn load_from_dir(dir: impl AsRef<Path>) -> Result<SlicerCredentials, CredentialError> {
    let dir = dir.as_ref();
    Ok(SlicerCredentials {
        cert_pem: read_optional(dir.join("slicer_cert.pem"))?,
        key_pem: read_optional(dir.join("slicer_key.pem"))?,
        crl_pem: read_optional(dir.join("slicer_crl.pem"))?,
    })
}

pub fn write_to_dir(
    dir: impl AsRef<Path>,
    creds: &SlicerCredentials,
) -> Result<PathBuf, CredentialError> {
    let dir = dir.as_ref();
    std::fs::create_dir_all(dir)?;
    if let Some(pem) = &creds.cert_pem {
        std::fs::write(dir.join("slicer_cert.pem"), pem)?;
    }
    if let Some(pem) = &creds.key_pem {
        std::fs::write(dir.join("slicer_key.pem"), pem)?;
    }
    if let Some(pem) = &creds.crl_pem {
        std::fs::write(dir.join("slicer_crl.pem"), pem)?;
    }
    Ok(dir.to_path_buf())
}

fn read_optional(path: PathBuf) -> Result<Option<String>, CredentialError> {
    match std::fs::read_to_string(&path) {
        Ok(s) if s.contains("BEGIN") => Ok(Some(s)),
        Ok(_) => Ok(None),
        Err(err) if err.kind() == std::io::ErrorKind::NotFound => Ok(None),
        Err(err) => Err(err.into()),
    }
}

/// Directories that may already hold Option B PEMs (OBN / Studio).
pub fn candidate_import_dirs() -> Vec<PathBuf> {
    let mut dirs = vec![default_config_dir()];
    if let Ok(xdg) = std::env::var("XDG_CONFIG_HOME") {
        let xdg = PathBuf::from(xdg);
        dirs.push(xdg.join("BambuStudio"));
        dirs.push(xdg.join("OrcaSlicer"));
    } else if let Ok(home) = std::env::var("HOME") {
        let home = PathBuf::from(home);
        dirs.push(home.join(".config/BambuStudio"));
        dirs.push(home.join(".config/OrcaSlicer"));
    }
    #[cfg(windows)]
    if let Ok(appdata) = std::env::var("APPDATA") {
        let appdata = PathBuf::from(appdata);
        dirs.push(appdata.join("BambuStudio"));
        dirs.push(appdata.join("OrcaSlicer"));
    }
    dirs
}

pub fn import_from_known_locations() -> Result<SlicerCredentials, CredentialError> {
    let mut merged = SlicerCredentials::default();
    for dir in candidate_import_dirs() {
        if !dir.is_dir() {
            continue;
        }
        let got = load_from_dir(&dir)?;
        if merged.cert_pem.is_none() {
            merged.cert_pem = got.cert_pem;
        }
        if merged.key_pem.is_none() {
            merged.key_pem = got.key_pem;
        }
        if merged.crl_pem.is_none() {
            merged.crl_pem = got.crl_pem;
        }
    }
    Ok(merged)
}
