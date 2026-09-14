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
        let maps = std::fs::read_to_string("/proc/self/maps").map_err(|err| err.to_string())?;
        let image = reconstruct_plugin(&maps, plugin)?;
        if image.is_empty() {
            return Err("no bambu_networking mappings after dlopen".into());
        }
        eprintln!("dumped {} bytes from plugin mappings", image.len());
        let mut out = File::create(dump).map_err(|err| err.to_string())?;
        out.write_all(&image).map_err(|err| err.to_string())?;
        Ok(())
    }

    fn reconstruct_plugin(maps: &str, plugin: &str) -> Result<Vec<u8>, String> {
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
                continue;
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
        let base = ranges[0].0;
        let last = ranges.last().unwrap().1;
        if last - base > 96 * 1024 * 1024 {
            return Err("plugin mapping too large".into());
        }
        let mut image = vec![0u8; (last - base) as usize];
        let mut mem = File::open("/proc/self/mem").map_err(|err| err.to_string())?;
        for (start, end) in ranges {
            let len = (end - start) as usize;
            if mem.seek(SeekFrom::Start(start)).is_err() {
                continue;
            }
            let mut buf = vec![0u8; len];
            match mem.read(&mut buf) {
                Ok(n) if n > 0 => {
                    buf.truncate(n);
                    let off = (start - base) as usize;
                    let n = buf.len().min(image.len().saturating_sub(off));
                    image[off..off + n].copy_from_slice(&buf[..n]);
                }
                _ => continue,
            }
        }
        Ok(image)
    }
}
