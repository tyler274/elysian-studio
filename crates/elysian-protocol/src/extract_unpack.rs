//! Spawn the isolated `elysian-vmp-dump` helper so VMProtect can self-decrypt.
//!
//! This crate never `dlopen`s the plugin. The helper is a separate process.

use std::io::Read;
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::time::{Duration, Instant};

use crate::credentials::{write_to_dir, CredentialError};
use crate::extract::ExtractReport;
use crate::extract_elf::{map_notes, scan_key_matching};

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
            "unpack: elysian-vmp-dump not on PATH (build -p elysian-vmp-dump); skipping self-unpack"
                .into(),
        );
        return Ok(());
    };
    let tmp = std::env::temp_dir().join(format!(
        "elysian-vmp-dump-{}-{}.bin",
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
    let key_out = tmp.with_extension("key.pem");
    let rand_out = tmp.with_extension("rand");
    let wrap_out = tmp.with_extension("wrap.pem");
    let secret_out = tmp.with_extension("secret.txt");
    let mut cmd = Command::new(&helper);
    cmd.arg(&plugin)
        .arg(&tmp)
        .env("BAMBU_VMP_KEY_OUT", &key_out)
        .env("BAMBU_VMP_RAND_OUT", &rand_out)
        .env("BAMBU_VMP_WRAP_OUT", &wrap_out)
        .env("BAMBU_VMP_SECRET_OUT", &secret_out)
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::piped());
    if let Some(hook) = find_vmp_hook() {
        report
            .notes
            .push(format!("unpack: LD_PRELOAD {}", hook.display()));
        prepend_ld_preload(&mut cmd, &hook);
    } else {
        report
            .notes
            .push("unpack: libbambu_vmp_hook.so not found; VMP anti-debug not masked".into());
    }
    let mut child = cmd
        .spawn()
        .map_err(|err| CredentialError::Message(format!("spawn elysian-vmp-dump: {err}")))?;
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
                let _ = std::fs::remove_file(&key_out);
                let _ = std::fs::remove_file(&rand_out);
                let _ = std::fs::remove_file(&wrap_out);
                let _ = std::fs::remove_file(&secret_out);
                return Ok(());
            }
            Ok(None) => std::thread::sleep(Duration::from_millis(50)),
            Err(err) => {
                report.notes.push(format!("unpack: wait helper: {err}"));
                let _ = std::fs::remove_file(&tmp);
                let _ = std::fs::remove_file(&key_out);
                let _ = std::fs::remove_file(&rand_out);
                let _ = std::fs::remove_file(&wrap_out);
                let _ = std::fs::remove_file(&secret_out);
                return Ok(());
            }
        }
    };
    let mut stderr = String::new();
    if let Some(mut pipe) = child.stderr.take() {
        let _ = pipe.read_to_string(&mut stderr);
    }
    if !stderr.trim().is_empty() {
        for line in stderr.lines().take(20) {
            report.notes.push(format!("unpack: {line}"));
        }
    }
    if !status.success() {
        report.notes.push(format!(
            "unpack: helper exited {status} (anti-debug/anti-VM or missing plugin); falling through"
        ));
        let _ = std::fs::remove_file(&tmp);
        let _ = std::fs::remove_file(&key_out);
        let _ = std::fs::remove_file(&rand_out);
        let _ = std::fs::remove_file(&wrap_out);
        let _ = std::fs::remove_file(&secret_out);
        return Ok(());
    }
    let bytes = match std::fs::read(&tmp) {
        Ok(b) => b,
        Err(err) => {
            report
                .notes
                .push(format!("unpack: could not read dump: {err}"));
            let _ = std::fs::remove_file(&tmp);
            let _ = std::fs::remove_file(&key_out);
            let _ = std::fs::remove_file(&rand_out);
            let _ = std::fs::remove_file(&wrap_out);
            let _ = std::fs::remove_file(&secret_out);
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
        let _ = std::fs::copy(&rand_out, dest.with_extension("rand"));
        report
            .notes
            .push(format!("unpack: copied dump to {}", dest.display()));
    }
    crate::extract::apply_appcert_dump(
        report,
        &bytes,
        &std::fs::read(&rand_out).unwrap_or_default(),
        "unpack",
    );
    report.notes.push(format!(
        "unpack: hook sidecars rand={} wrap={} secret={}",
        std::fs::metadata(&rand_out).map(|m| m.len()).unwrap_or(0),
        std::fs::metadata(&wrap_out).map(|m| m.len()).unwrap_or(0),
        std::fs::metadata(&secret_out).map(|m| m.len()).unwrap_or(0),
    ));
    if let Ok(raw) = std::fs::read(&secret_out) {
        if raw.starts_with(b"GLOF") && raw.len() >= 40 {
            report.credentials.client_auth_secret = Some(raw);
            report
                .notes
                .push("unpack: captured client_auth_secret from in-process hook".into());
        }
    }
    if let Ok(raw) = std::fs::read(&wrap_out) {
        if let Some(pem) = crate::extract_bootstrap::wrap_bytes_to_pem(&raw) {
            report.credentials.server_wrap_pem = Some(pem);
            report
                .notes
                .push("unpack: captured server_wrap_key from in-process hook".into());
        }
    }
    if let Ok(pem) = std::fs::read_to_string(&key_out) {
        if crate::signing::load_private_key(&pem).is_ok() {
            report.credentials.key_pem = Some(pem);
            report
                .notes
                .push("unpack: captured private key from in-process hook".into());
        }
    }
    if report.credentials.key_pem.is_none() {
        if let Some(pem) = crate::extract_appcert::try_unwrap_appcert_key(
            &bytes,
            report.credentials.cert_pem.as_deref(),
        ) {
            report.credentials.key_pem = Some(pem);
            report
                .notes
                .push("unpack: unwrapped get_app_cert key blob (custom AES-CTR, not VMP)".into());
        }
    }
    if report.credentials.key_pem.is_none() {
        if let Ok(rands) = std::fs::read(&rand_out) {
            if let Some(pem) = crate::extract_elf::try_decrypt_app_key(&bytes, &rands) {
                report.credentials.key_pem = Some(pem);
                report.notes.push(
                    "unpack: decrypted get_app_cert key blob with captured session key".into(),
                );
            }
        }
    }
    if report.credentials.key_pem.is_none() {
        if let Some(cert) = report.credentials.cert_pem.clone() {
            if let Some(key) = scan_key_matching(&bytes, &cert) {
                report.credentials.key_pem = Some(key);
                report
                    .notes
                    .push("unpack: recovered private key matching on-disk cert".into());
            } else {
                report.notes.push(
                    "unpack: app cert/CRL present in dump; private key still not in PEM/DER (plugin encrypts that field)"
                        .into(),
                );
            }
        }
    }
    if report.credentials.cert_pem.is_some()
        || report.credentials.key_pem.is_some()
        || report.credentials.crl_pem.is_some()
        || report.credentials.client_auth_secret.is_some()
        || report.credentials.server_wrap_pem.is_some()
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
    let _ = std::fs::remove_file(&key_out);
    let _ = std::fs::remove_file(&rand_out);
    let _ = std::fs::remove_file(&wrap_out);
    let _ = std::fs::remove_file(&secret_out);
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
            let candidate = dir.join("elysian-vmp-dump");
            if candidate.is_file() {
                return Some(candidate);
            }
        }
    }
    let path = std::env::var_os("PATH")?;
    std::env::split_paths(&path).find_map(|dir| {
        let candidate = dir.join("elysian-vmp-dump");
        candidate.is_file().then_some(candidate)
    })
}

pub fn find_vmp_hook() -> Option<PathBuf> {
    if let Ok(p) = std::env::var("BAMBU_VMP_HOOK") {
        let p = PathBuf::from(p);
        if p.is_file() {
            return Some(p);
        }
    }
    let name = "libbambu_vmp_hook.so";
    if let Some(helper) = find_vmp_dump() {
        let candidate = helper.with_file_name(name);
        if candidate.is_file() {
            return Some(candidate);
        }
        if let Some(dir) = helper.parent() {
            let candidate = dir.join(name);
            if candidate.is_file() {
                return Some(candidate);
            }
        }
    }
    if let Ok(exe) = std::env::current_exe() {
        if let Some(dir) = exe.parent() {
            let candidate = dir.join(name);
            if candidate.is_file() {
                return Some(candidate);
            }
        }
    }
    let path = std::env::var_os("PATH")?;
    std::env::split_paths(&path).find_map(|dir| {
        let candidate = dir.join(name);
        candidate.is_file().then_some(candidate)
    })
}

pub(crate) fn prepend_ld_preload(cmd: &mut Command, hook: &Path) {
    let mut preload = hook.as_os_str().to_os_string();
    if let Ok(old) = std::env::var("LD_PRELOAD") {
        if !old.is_empty() {
            preload.push(":");
            preload.push(old);
        }
    }
    cmd.env("LD_PRELOAD", preload);
}
