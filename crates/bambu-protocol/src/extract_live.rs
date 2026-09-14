//! Harvest Option B PEMs from a sandboxed official `bambu-studio` child.
//!
//! The rewrite never `dlopen`s `libbambu_networking`. Official Studio is a
//! separate process; this module only reads the child's `/proc/<pid>/mem` (parent
//! of the child, so Yama `ptrace_scope=1` still allows it) and writes `slicer_*.pem`.
//! Bodies are never logged.

use std::fs::{self, File};
use std::io::{Read, Seek, SeekFrom};
use std::path::{Path, PathBuf};
use std::process::{Child, Command, Stdio};
use std::time::{Duration, Instant};

use crate::cloud::load_cloud_session;
use crate::credentials::{default_config_dir, write_to_dir, CredentialError, SlicerCredentials};
use crate::extract::{extract_pems_plain, merge_creds, ExtractReport};
use crate::extract_elf::{harvest_maps, parse_proc_maps};
use crate::signing::{load_private_key, slicer_cert_id};
use crate::studio_import::default_studio_data_dir;

const MINIMAL_STUDIO_CONF: &str = r#"{
  "app": {
    "installed_networking": "1",
    "update_network_plugin": "false",
    "ignore_module_cert": "1"
  }
}
"#;

pub fn engine_conf_json(access_token: &str, refresh_token: &str, region: &str) -> String {
    let country = match region.trim().to_ascii_lowercase().as_str() {
        "cn" | "china" => "CN",
        _ => "US",
    };
    serde_json::json!({
        "accessToken": access_token,
        "refreshToken": refresh_token,
        "country_code": country,
    })
    .to_string()
}

pub fn extract_live(
    report: &mut ExtractReport,
    plugin: Option<&Path>,
    out_dir: Option<&Path>,
    timeout: Duration,
) -> Result<(), CredentialError> {
    #[cfg(not(target_os = "linux"))]
    {
        let _ = (plugin, out_dir, timeout);
        report.notes.push(
            "live Studio extract is Linux-only (/proc/<pid>/mem); copy slicer_*.pem into the config dir"
                .into(),
        );
        return Ok(());
    }
    #[cfg(target_os = "linux")]
    extract_live_linux(report, plugin, out_dir, timeout)
}

#[cfg(target_os = "linux")]
fn extract_live_linux(
    report: &mut ExtractReport,
    plugin: Option<&Path>,
    out_dir: Option<&Path>,
    timeout: Duration,
) -> Result<(), CredentialError> {
    if std::env::var_os("WAYLAND_DISPLAY").is_none() && std::env::var_os("DISPLAY").is_none() {
        return Err(CredentialError::Message(
            "live extract needs a graphical session (WAYLAND_DISPLAY or DISPLAY); official Studio does not load the network plugin under --slice"
                .into(),
        ));
    }
    let studio_bin = find_studio_bin().ok_or_else(|| {
        CredentialError::Message(
            "bambu-studio not on PATH; set BAMBU_STUDIO to the official binary".into(),
        )
    })?;
    let plugin_file = plugin
        .map(Path::to_path_buf)
        .or_else(crate::extract::find_stock_plugin)
        .ok_or_else(|| {
            CredentialError::Message("no libbambu_networking plugin found for the sandbox".into())
        })?;
    let dest = out_dir
        .map(Path::to_path_buf)
        .unwrap_or_else(default_config_dir);
    let sandbox = std::env::temp_dir().join(format!(
        "bambu-live-extract-{}-{}",
        std::process::id(),
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map(|d| d.as_millis())
            .unwrap_or(0)
    ));
    prepare_sandbox(&sandbox, &plugin_file, &dest, report)?;
    report.notes.push(format!(
        "sandbox HOME {} (official Studio, no dlopen in this process)",
        sandbox.display()
    ));
    let mut child = spawn_studio(&studio_bin, &sandbox, report)?;
    let seed_deadline = Instant::now() + timeout;
    let interactive_deadline = seed_deadline + timeout;
    let mut asked_login = false;
    let engine_path = sandbox
        .join(".config")
        .join("BambuStudio")
        .join("BambuNetworkEngine.conf");
    let mut creds = SlicerCredentials::default();
    let mut engine_len = fs::metadata(&engine_path).map(|m| m.len()).unwrap_or(0);
    loop {
        match child.try_wait() {
            Ok(Some(status)) => {
                report.notes.push(format!(
                    "official Studio exited ({status}) before PEMs appeared"
                ));
                break;
            }
            Ok(None) => {}
            Err(err) => {
                report.notes.push(format!("wait on Studio: {err}"));
                break;
            }
        }
        for pid in descendant_pids(child.id()) {
            merge_creds(&mut creds, harvest_pid(pid));
        }
        creds = validate_creds(&creds);
        if creds.has_cert_and_key() {
            report
                .notes
                .push("harvested PEMs from Studio process memory".into());
            break;
        }
        let now = Instant::now();
        if !asked_login && now >= seed_deadline {
            asked_login = true;
            report.notes.push(
                "seeded cloud tokens did not decrypt PEMs; sign in inside the sandboxed Studio window"
                    .into(),
            );
        }
        if now >= interactive_deadline {
            report
                .notes
                .push("live extract timed out without slicer_key.pem".into());
            break;
        }
        if asked_login {
            if let Ok(meta) = fs::metadata(&engine_path) {
                if meta.len() != engine_len {
                    engine_len = meta.len();
                    report
                        .notes
                        .push("sandbox Studio updated engine.conf; still scanning memory".into());
                }
            }
        }
        std::thread::sleep(Duration::from_millis(400));
    }
    let _ = child.kill();
    let _ = child.wait();
    merge_creds(&mut report.credentials, creds);
    if report.credentials.cert_pem.is_some()
        || report.credentials.key_pem.is_some()
        || report.credentials.crl_pem.is_some()
    {
        write_to_dir(&dest, &report.credentials)?;
        report
            .notes
            .push(format!("wrote credentials under {}", dest.display()));
    }
    let _ = fs::remove_dir_all(&sandbox);
    Ok(())
}

fn prepare_sandbox(
    sandbox: &Path,
    plugin_file: &Path,
    rewrite_config: &Path,
    report: &mut ExtractReport,
) -> Result<(), CredentialError> {
    let studio_cfg = sandbox.join(".config").join("BambuStudio");
    let plugins = studio_cfg.join("plugins");
    fs::create_dir_all(&plugins)?;
    copy_plugin_bundle(plugin_file, &plugins, report)?;
    let conf_dest = studio_cfg.join("BambuStudio.conf");
    let user_conf = default_studio_data_dir().join("BambuStudio.conf");
    if user_conf.is_file() {
        fs::copy(&user_conf, &conf_dest)?;
    } else {
        fs::write(&conf_dest, MINIMAL_STUDIO_CONF)?;
    }
    let session = load_cloud_session(rewrite_config).unwrap_or_default();
    let engine = engine_conf_json(
        &session.access_token,
        &session.refresh_token,
        &session.region,
    );
    fs::write(studio_cfg.join("BambuNetworkEngine.conf"), engine)?;
    if session.access_token.is_empty() {
        report
            .notes
            .push("no cloud_token to seed; Studio login in the sandbox window is required".into());
    } else {
        report
            .notes
            .push("seeded BambuNetworkEngine.conf from rewrite cloud tokens (not printed)".into());
    }
    Ok(())
}

fn copy_plugin_bundle(
    plugin_file: &Path,
    dest: &Path,
    report: &mut ExtractReport,
) -> Result<(), CredentialError> {
    let Some(src_dir) = plugin_file.parent() else {
        return Err(CredentialError::Message("plugin path has no parent".into()));
    };
    let Ok(entries) = fs::read_dir(src_dir) else {
        fs::copy(
            plugin_file,
            dest.join(plugin_file.file_name().unwrap_or_default()),
        )?;
        return Ok(());
    };
    for entry in entries.flatten() {
        let path = entry.path();
        if !path.is_file() {
            continue;
        }
        let name = path
            .file_name()
            .and_then(|n| n.to_str())
            .unwrap_or("")
            .to_ascii_lowercase();
        if name.contains("bambu_networking")
            || name.contains("bambusource")
            || name.contains("live555")
            || name.contains("agora")
        {
            if let Some(file) = path.file_name() {
                fs::copy(&path, dest.join(file))?;
            }
        }
    }
    report
        .notes
        .push(format!("copied plugin bundle from {}", src_dir.display()));
    Ok(())
}

fn spawn_studio(
    studio_bin: &Path,
    sandbox: &Path,
    report: &mut ExtractReport,
) -> Result<Child, CredentialError> {
    let mut cmd = if let Some(bwrap) = find_on_path("bwrap") {
        report
            .notes
            .push(format!("launching via {}", bwrap.display()));
        let mut cmd = Command::new(bwrap);
        cmd.arg("--die-with-parent")
            .arg("--unshare-all")
            .arg("--share-net")
            .arg("--proc")
            .arg("/proc")
            .arg("--dev")
            .arg("/dev")
            .arg("--tmpfs")
            .arg("/tmp");
        for host in [
            Path::new("/nix"),
            Path::new("/etc"),
            Path::new("/run"),
            Path::new("/sys"),
            Path::new("/usr"),
            Path::new("/bin"),
            Path::new("/lib"),
            Path::new("/lib64"),
        ] {
            ro_bind_if_exists(&mut cmd, host);
        }
        cmd.arg("--bind").arg(sandbox).arg(sandbox);
        bind_session_sockets(&mut cmd);
        pass_gui_env(&mut cmd);
        cmd.env("HOME", sandbox)
            .env("XDG_CONFIG_HOME", sandbox.join(".config"))
            .env("XDG_CACHE_HOME", sandbox.join(".cache"))
            .arg("--chdir")
            .arg(sandbox)
            .arg("--")
            .arg(studio_bin);
        cmd
    } else {
        report
            .notes
            .push("bwrap not on PATH; running official Studio with isolated HOME only".into());
        let mut cmd = Command::new(studio_bin);
        pass_gui_env(&mut cmd);
        cmd.env("HOME", sandbox)
            .env("XDG_CONFIG_HOME", sandbox.join(".config"))
            .env("XDG_CACHE_HOME", sandbox.join(".cache"));
        cmd
    };
    cmd.stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null());
    cmd.spawn()
        .map_err(|err| CredentialError::Message(format!("spawn official bambu-studio: {err}")))
}

fn ro_bind_if_exists(cmd: &mut Command, host: &Path) {
    if host.exists() {
        cmd.arg("--ro-bind").arg(host).arg(host);
    }
}

fn bind_session_sockets(cmd: &mut Command) {
    if let Ok(runtime) = std::env::var("XDG_RUNTIME_DIR") {
        let runtime = PathBuf::from(runtime);
        ro_bind_if_exists(cmd, &runtime);
        cmd.env("XDG_RUNTIME_DIR", &runtime);
    }
    if let Ok(auth) = std::env::var("XAUTHORITY") {
        let auth_path = PathBuf::from(&auth);
        ro_bind_if_exists(cmd, &auth_path);
        cmd.env("XAUTHORITY", auth);
    }
    let x11 = Path::new("/tmp/.X11-unix");
    ro_bind_if_exists(cmd, x11);
}

fn pass_gui_env(cmd: &mut Command) {
    for key in [
        "DISPLAY",
        "WAYLAND_DISPLAY",
        "XDG_SESSION_TYPE",
        "DBUS_SESSION_BUS_ADDRESS",
        "QT_QPA_PLATFORM",
        "GDK_BACKEND",
        "LANG",
        "LC_ALL",
        "XDG_CURRENT_DESKTOP",
    ] {
        if let Ok(val) = std::env::var(key) {
            cmd.env(key, val);
        }
    }
}

fn find_studio_bin() -> Option<PathBuf> {
    if let Ok(p) = std::env::var("BAMBU_STUDIO") {
        let p = PathBuf::from(p);
        if p.is_file() {
            return Some(p);
        }
    }
    find_on_path("bambu-studio")
}

fn find_on_path(name: &str) -> Option<PathBuf> {
    let path = std::env::var_os("PATH")?;
    std::env::split_paths(&path).find_map(|dir| {
        let candidate = dir.join(name);
        candidate.is_file().then_some(candidate)
    })
}

/// Visible pids: the spawned child plus descendants (bwrap may stay as parent).
pub fn descendant_pids(root: u32) -> Vec<u32> {
    let mut out = vec![root];
    let mut i = 0;
    while i < out.len() {
        let parent = out[i];
        if let Ok(entries) = fs::read_dir("/proc") {
            for entry in entries.flatten() {
                let name = entry.file_name();
                let Some(pid) = name.to_str().and_then(|s| s.parse::<u32>().ok()) else {
                    continue;
                };
                if out.contains(&pid) {
                    continue;
                }
                let Ok(stat) = fs::read_to_string(entry.path().join("stat")) else {
                    continue;
                };
                if parse_ppid(&stat) == Some(parent) {
                    out.push(pid);
                }
            }
        }
        i += 1;
    }
    out
}

pub fn parse_ppid(stat: &str) -> Option<u32> {
    let close = stat.rfind(')')?;
    let rest = stat.get(close + 1..)?.trim_start();
    let mut parts = rest.split_whitespace();
    let _state = parts.next()?;
    parts.next()?.parse().ok()
}

pub fn harvest_pid(pid: u32) -> SlicerCredentials {
    let mut creds = SlicerCredentials::default();
    let Ok(maps_text) = fs::read_to_string(format!("/proc/{pid}/maps")) else {
        return creds;
    };
    let maps = parse_proc_maps(&maps_text);
    let Ok(mut mem) = File::open(format!("/proc/{pid}/mem")) else {
        return creds;
    };
    for map in harvest_maps(&maps) {
        harvest_range(&mut mem, map.start, map.end, &mut creds);
        if creds.has_cert_and_key() && creds.crl_pem.is_some() {
            break;
        }
    }
    creds
}

fn harvest_range(mem: &mut File, start: u64, end: u64, creds: &mut SlicerCredentials) {
    const CHUNK: u64 = 1024 * 1024;
    const OVERLAP: usize = 80;
    let mut offset = start;
    let mut carry = Vec::new();
    while offset < end {
        let take = (end - offset).min(CHUNK) as usize;
        if mem.seek(SeekFrom::Start(offset)).is_err() {
            break;
        }
        let mut buf = vec![0u8; take];
        let n = match mem.read(&mut buf) {
            Ok(0) => break,
            Ok(n) => n,
            Err(_) => break,
        };
        buf.truncate(n);
        let mut combined = carry;
        combined.extend_from_slice(&buf);
        merge_creds(creds, extract_pems_plain(&combined));
        if creds.has_cert_and_key() {
            return;
        }
        let keep = combined.len().min(OVERLAP);
        carry = combined[combined.len() - keep..].to_vec();
        offset += n as u64;
    }
}

pub fn validate_creds(creds: &SlicerCredentials) -> SlicerCredentials {
    let mut out = SlicerCredentials::default();
    if let Some(cert) = &creds.cert_pem {
        if slicer_cert_id(cert).is_ok() {
            out.cert_pem = Some(cert.clone());
        }
    }
    if let Some(key) = &creds.key_pem {
        if load_private_key(key).is_ok() {
            out.key_pem = Some(key.clone());
        }
    }
    if let Some(crl) = &creds.crl_pem {
        if crl.to_ascii_uppercase().contains("BEGIN") {
            out.crl_pem = Some(crl.clone());
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn engine_conf_seeds_us_and_cn() {
        let us = engine_conf_json("tok", "ref", "us");
        assert!(us.contains("\"country_code\":\"US\""));
        assert!(us.contains("\"accessToken\":\"tok\""));
        let cn = engine_conf_json("", "", "cn");
        assert!(cn.contains("\"country_code\":\"CN\""));
        assert!(!cn.contains("cloud_token"));
    }

    #[test]
    fn parse_ppid_skips_comm_parens() {
        let stat =
            "4091 (bambu-studio) S 4080 4091 4091 0 -1 4194304 0 0 0 0 0 0 0 0 20 0 1 0 0 0 0";
        assert_eq!(parse_ppid(stat), Some(4080));
    }

    #[test]
    fn harvest_maps_keeps_heap_anon_and_rx_plugin() {
        let maps = parse_proc_maps(
            "7f000000-7f001000 r-xp 00000000 00:00 0 /usr/lib/libc.so.6\n\
             7f100000-7f180000 rw-p 00000000 00:00 0 [heap]\n\
             7f1a0000-7f1b0000 r-xp 00000000 00:00 1 /home/u/.config/BambuStudio/plugins/libbambu_networking.so\n\
             7f200000-7f210000 rw-p 00000000 00:00 0 /home/u/.config/BambuStudio/plugins/libbambu_networking.so\n\
             7f300000-7f400000 rw-p 00000000 00:00 0\n",
        );
        let ranges = harvest_maps(&maps);
        assert_eq!(ranges.len(), 4, "{ranges:?}");
        assert!(crate::extract_elf::plugin_readable_maps(&maps)
            .iter()
            .any(|m| m.perms.contains("r-x")));
    }

    #[test]
    #[cfg(target_os = "linux")]
    fn harvest_finds_heap_pems() {
        let cert = include_str!("../tests/fixtures/test_slicer_cert.pem");
        let key = include_str!("../tests/fixtures/test_slicer_key.pem");
        let mut held = vec![0u8; 4096];
        held.extend_from_slice(cert.as_bytes());
        held.extend_from_slice(b"\n");
        held.extend_from_slice(key.as_bytes());
        std::hint::black_box(&held);
        let found = harvest_pid(std::process::id());
        let found = validate_creds(&found);
        assert!(
            found.has_cert_and_key(),
            "heap PEM harvest missed fixture cert/key"
        );
        drop(held);
    }
}
