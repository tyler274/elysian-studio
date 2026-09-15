//! Isolated helper: `dlopen` a local `libbambu_networking` so VMProtect can
//! self-decrypt, then dump the in-memory mappings. The protocol crate never
//! loads this plugin.

#![allow(unsafe_code)]

use std::env;
use std::process::ExitCode;

fn main() -> ExitCode {
    let mut args = env::args().skip(1);
    let Some(plugin) = args.next() else {
        eprintln!("usage: bambu-vmp-dump <plugin.so> <dump.bin>");
        return ExitCode::from(2);
    };
    let Some(dump) = args.next() else {
        eprintln!("usage: bambu-vmp-dump <plugin.so> <dump.bin>");
        return ExitCode::from(2);
    };
    match run(&plugin, &dump) {
        Ok(()) => ExitCode::SUCCESS,
        Err(err) => {
            eprintln!("{err}");
            ExitCode::from(1)
        }
    }
}

#[cfg(not(target_os = "linux"))]
fn run(_plugin: &str, _dump: &str) -> Result<(), String> {
    Err("bambu-vmp-dump is Linux-only".into())
}

#[cfg(target_os = "linux")]
fn run(plugin: &str, dump: &str) -> Result<(), String> {
    linux::run(plugin, dump)
}

#[cfg(target_os = "linux")]
mod linux {
    use std::ffi::{c_char, c_int, c_void, CStr, CString};
    use std::fs::File;
    use std::io::{Read, Seek, SeekFrom, Write};
    use std::path::Path;

    const RTLD_NOW: c_int = 2;

    #[link(name = "dl")]
    extern "C" {
        fn dlopen(filename: *const c_char, flags: c_int) -> *mut c_void;
        fn dlsym(handle: *mut c_void, symbol: *const c_char) -> *mut c_void;
        fn dlerror() -> *mut c_char;
    }

    pub(super) fn run(plugin: &str, dump: &str) -> Result<(), String> {
        unsafe {
            extern "C" {
                fn prctl(option: c_int, arg2: *const c_char) -> c_int;
            }
            const PR_SET_NAME: c_int = 15;
            let _ = prctl(PR_SET_NAME, b"bambu-studio\0".as_ptr().cast());
        }
        let c_path = CString::new(plugin).map_err(|err| err.to_string())?;
        let handle = unsafe { dlopen(c_path.as_ptr(), RTLD_NOW) };
        if handle.is_null() {
            let msg = unsafe { dlerror() };
            let detail = if msg.is_null() {
                "dlopen failed".into()
            } else {
                unsafe { CStr::from_ptr(msg) }
                    .to_string_lossy()
                    .into_owned()
            };
            return Err(detail);
        }
        let sym = CString::new("bambu_network_get_version").unwrap();
        let _ = unsafe { dlsym(handle, sym.as_ptr()) };

        let cfg = std::env::temp_dir().join(format!("bambu-vmp-agent-{}", std::process::id()));
        let _ = std::fs::create_dir_all(&cfg);
        if let Ok(home) = std::env::var("HOME") {
            let engine =
                std::path::Path::new(&home).join(".config/BambuStudio/BambuNetworkEngine.conf");
            if engine.is_file() {
                let _ = std::fs::copy(&engine, cfg.join("BambuNetworkEngine.conf"));
                eprintln!("copied official BambuNetworkEngine.conf into helper config");
            }
        }
        let cert_folder = find_cert_folder();
        if let Some(ref cert) = cert_folder {
            eprintln!(
                "init agent config={} cert={}",
                cfg.display(),
                cert.display()
            );
        } else {
            eprintln!("init agent without slicer_base64.cer (set BAMBU_STUDIO_RESOURCES)");
        }
        let c_cfg = CString::new(cfg.to_string_lossy().as_ref()).map_err(|err| err.to_string())?;
        let c_cert = CString::new(
            cert_folder
                .as_ref()
                .map(|p| p.to_string_lossy().into_owned())
                .unwrap_or_default(),
        )
        .map_err(|err| err.to_string())?;
        let rc = unsafe { vmp_init_agent(handle, c_cfg.as_ptr(), c_cert.as_ptr()) };
        eprintln!("vmp_init_agent rc={rc}");

        let maps = std::fs::read_to_string("/proc/self/maps").map_err(|err| err.to_string())?;
        let image = reconstruct_interesting(&maps, plugin)?;
        if image.is_empty() {
            return Err("no plugin/heap mappings after dlopen".into());
        }
        eprintln!("dumped {} bytes from plugin+heap mappings", image.len());
        let mut out = File::create(dump).map_err(|err| err.to_string())?;
        out.write_all(&image).map_err(|err| err.to_string())?;
        extern "C" {
            fn _exit(code: c_int) -> !;
        }
        unsafe { _exit(0) };
    }

    fn find_cert_folder() -> Option<std::path::PathBuf> {
        if let Ok(p) = std::env::var("BAMBU_STUDIO_CERT") {
            let p = std::path::PathBuf::from(p);
            if p.join("slicer_base64.cer").is_file() {
                return Some(p);
            }
            if p.is_file() {
                return p.parent().map(std::path::Path::to_path_buf);
            }
        }
        if let Ok(p) = std::env::var("BAMBU_STUDIO_RESOURCES") {
            let c = std::path::PathBuf::from(p).join("cert");
            if c.join("slicer_base64.cer").is_file() {
                return Some(c);
            }
        }
        let path = std::env::var_os("PATH")?;
        for dir in std::env::split_paths(&path) {
            let bin = dir.join("bambu-studio");
            if !bin.is_file() {
                continue;
            }
            if let Some(parent) = bin.parent() {
                for rel in [
                    "share/BambuStudio/cert",
                    "../share/BambuStudio/cert",
                    "../../share/BambuStudio/cert",
                ] {
                    let c = parent.join(rel);
                    if c.join("slicer_base64.cer").is_file() {
                        return Some(c);
                    }
                }
            }
        }
        None
    }

    extern "C" {
        fn vmp_init_agent(
            handle: *mut c_void,
            config_dir: *const c_char,
            cert_folder: *const c_char,
        ) -> c_int;
    }

    fn reconstruct_interesting(maps: &str, plugin: &str) -> Result<Vec<u8>, String> {
        let plugin_name = Path::new(plugin)
            .file_name()
            .and_then(|n| n.to_str())
            .unwrap_or("bambu_networking");
        let mut ranges: Vec<(u64, u64)> = Vec::new();
        for line in maps.lines() {
            let mut parts = line.split_whitespace();
            let Some(range) = parts.next() else {
                continue;
            };
            let Some(perms) = parts.next() else {
                continue;
            };
            let pathname = parts.nth(3).unwrap_or("");
            if !perms.contains('r') {
                continue;
            }
            if !pathname.contains("bambu_networking")
                && !pathname.contains("bambunetwork")
                && !pathname.contains(plugin_name)
            {
                let anon = pathname.is_empty()
                    || pathname == "[heap]"
                    || pathname == "[stack]"
                    || pathname.starts_with("[anon")
                    || pathname.contains("memfd")
                    || pathname.contains("libcrypto")
                    || pathname.contains("libssl");
                if !(perms.starts_with("rw") && anon) {
                    continue;
                }
            }
            let Some((start, end)) = range.split_once('-') else {
                continue;
            };
            let start = u64::from_str_radix(start, 16).map_err(|err| err.to_string())?;
            let end = u64::from_str_radix(end, 16).map_err(|err| err.to_string())?;
            if end > start {
                ranges.push((start, end));
            }
        }
        ranges.sort_by_key(|r| r.0);
        if ranges.is_empty() {
            return Ok(Vec::new());
        }
        let mut image = Vec::new();
        let mut mem = File::open("/proc/self/mem").map_err(|err| err.to_string())?;
        for (start, end) in ranges {
            let len = (end - start) as usize;
            if image.len().saturating_add(len) > 96 * 1024 * 1024 {
                break;
            }
            if mem.seek(SeekFrom::Start(start)).is_err() {
                continue;
            }
            let mut buf = vec![0u8; len];
            match mem.read(&mut buf) {
                Ok(n) if n > 0 => {
                    buf.truncate(n);
                    image.extend_from_slice(&buf);
                }
                _ => continue,
            }
        }
        Ok(image)
    }
}
