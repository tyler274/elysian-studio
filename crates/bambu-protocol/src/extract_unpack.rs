//! Spawn the isolated `bambu-vmp-dump` helper so VMProtect can self-decrypt.
//!
//! This crate never `dlopen`s the plugin. The helper is a separate process.

use std::io::Read;
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::time::{Duration, Instant};

use crate::credentials::{write_to_dir, CredentialError};
use crate::extract::{merge_creds, ExtractReport};
use crate::extract_elf::{map_notes, scan_image};

pub fn extract_unpack(
    report: &mut ExtractReport,
    plugin: Option<&Path>,
    out_dir: Option<&Path>,
    dump_elf: Option<&Path>,
    timeout: Duration,
) -> Result<(), CredentialError> {
    let plugin = plugin
        .map(Path::to_path_buf)
        .or_else(|| report.plugin.clone())
        .or_else(crate::extract::find_stock_plugin);
    let Some(plugin) = plugin else {
        report
            .notes
            .push("unpack: no libbambu_networking plugin path".into());
        return Ok(());
    };
    report.notes.extend(map_notes(&plugin));
    let Some(helper) = find_vmp_dump() else {
        report.notes.push(
            "unpack: bambu-vmp-dump not on PATH (build -p bambu-vmp-dump); skipping self-unpack"
                .into(),
        );
        return Ok(());
    };
    let tmp = std::env::temp_dir().join(format!(
        "bambu-vmp-dump-{}-{}.bin",
        std::process::id(),
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map(|d| d.as_millis())
            .unwrap_or(0)
    ));
    report.notes.push(format!(
        "unpack: spawning {} on {}",
        helper.display(),
        plugin.display()
    ));
    let mut child = Command::new(&helper)
        .arg(&plugin)
        .arg(&tmp)
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::piped())
        .spawn()
        .map_err(|err| CredentialError::Message(format!("spawn bambu-vmp-dump: {err}")))?;
    let deadline = Instant::now() + timeout.max(Duration::from_secs(5));
    let status = loop {
        match child.try_wait() {
            Ok(Some(status)) => break status,
            Ok(None) if Instant::now() >= deadline => {
                let _ = child.kill();
                let _ = child.wait();
                report
                    .notes
                    .push("unpack: helper timed out (anti-debug/anti-VM?); falling through".into());
                let _ = std::fs::remove_file(&tmp);
                return Ok(());
            }
            Ok(None) => std::thread::sleep(Duration::from_millis(50)),
            Err(err) => {
                report.notes.push(format!("unpack: wait helper: {err}"));
                let _ = std::fs::remove_file(&tmp);
                return Ok(());
            }
        }
    };
    let mut stderr = String::new();
    if let Some(mut pipe) = child.stderr.take() {
        let _ = pipe.read_to_string(&mut stderr);
    }
    if !stderr.trim().is_empty() {
        for line in stderr.lines().take(8) {
            report.notes.push(format!("unpack: {line}"));
        }
    }
    if !status.success() {
        report.notes.push(format!(
            "unpack: helper exited {status} (anti-debug/anti-VM or missing plugin); falling through"
        ));
        let _ = std::fs::remove_file(&tmp);
        return Ok(());
    }
    let bytes = match std::fs::read(&tmp) {
        Ok(b) => b,
        Err(err) => {
            report
                .notes
                .push(format!("unpack: could not read dump: {err}"));
            return Ok(());
        }
    };
    report
        .notes
        .push(format!("unpack: dumped {} bytes", bytes.len()));
    if let Some(dest) = dump_elf {
        if let Some(parent) = dest.parent() {
            if !parent.as_os_str().is_empty() {
                let _ = std::fs::create_dir_all(parent);
            }
        }
        std::fs::copy(&tmp, dest)?;
        report
            .notes
            .push(format!("unpack: copied dump to {}", dest.display()));
    }
    let found = scan_image(&bytes);
    merge_creds(&mut report.credentials, found);
    if report.credentials.cert_pem.is_some()
        || report.credentials.key_pem.is_some()
        || report.credentials.crl_pem.is_some()
    {
        let dest = out_dir
            .map(Path::to_path_buf)
            .unwrap_or_else(crate::credentials::default_config_dir);
        write_to_dir(&dest, &report.credentials)?;
        report
            .notes
            .push(format!("wrote credentials under {}", dest.display()));
    } else {
        report
            .notes
            .push("unpack: dump had no usable PEM/DER identity".into());
    }
    let _ = std::fs::remove_file(&tmp);
    Ok(())
}

pub fn find_vmp_dump() -> Option<PathBuf> {
    if let Ok(p) = std::env::var("BAMBU_VMP_DUMP") {
        let p = PathBuf::from(p);
        if p.is_file() {
            return Some(p);
        }
    }
    if let Ok(exe) = std::env::current_exe() {
        if let Some(dir) = exe.parent() {
            let candidate = dir.join("bambu-vmp-dump");
            if candidate.is_file() {
                return Some(candidate);
            }
        }
    }
    let path = std::env::var_os("PATH")?;
    std::env::split_paths(&path).find_map(|dir| {
        let candidate = dir.join("bambu-vmp-dump");
        candidate.is_file().then_some(candidate)
    })
}
