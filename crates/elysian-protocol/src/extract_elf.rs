//! Map a packed ELF64 plugin (PT_LOAD, entropy, dynsym) without decrypting VMProtect.
//!
//! Notes never include key material. Scanning decrypted dumps for PEMs/DER lives
//! here so the protocol crate can harvest an image the helper reconstructed.

use std::path::Path;

use aes_gcm::aead::{Aead, KeyInit};
use aes_gcm::{Aes256Gcm, Nonce};
use base64::Engine as _;
use hkdf::Hkdf;
use rsa::pkcs1::DecodeRsaPrivateKey;
use rsa::pkcs8::{DecodePrivateKey, EncodePrivateKey, LineEnding};
use rsa::traits::PublicKeyParts;
use rsa::{BigUint, RsaPrivateKey};
use sha2::Sha256;
use x509_parser::prelude::{FromDer, X509Certificate};
use x509_parser::revocation_list::CertificateRevocationList;

use crate::credentials::SlicerCredentials;
use crate::extract::{extract_pems_plain, merge_creds};
use crate::signing::{load_private_key, public_key_from_cert_pem, slicer_cert_id};

const ELF_MAGIC: &[u8] = b"\x7fELF";
const PT_LOAD: u32 = 1;
const SHT_SYMTAB: u32 = 2;
const SHT_DYNSYM: u32 = 11;
const STT_FUNC: u8 = 2;
const STT_OBJECT: u8 = 1;

#[derive(Debug, Clone, Default)]
pub struct ElfMap {
    pub file_size: usize,
    pub pt_load: Vec<PtLoad>,
    pub sections: Vec<String>,
    pub network_exports: Vec<String>,
    pub notes: Vec<String>,
}

#[derive(Debug, Clone)]
pub struct PtLoad {
    pub index: usize,
    pub vaddr: u64,
    pub memsz: u64,
    pub filesz: u64,
    pub entropy: f64,
}

impl PtLoad {
    fn note(&self) -> String {
        format!(
            "PT_LOAD[{}] vaddr=0x{:x} memsz={} filesz={} entropy={:.2}",
            self.index, self.vaddr, self.memsz, self.filesz, self.entropy
        )
    }
}

#[derive(Debug, Clone)]
pub struct ProcMap {
    pub start: u64,
    pub end: u64,
    pub perms: String,
    pub pathname: String,
}

pub fn map_elf_file(path: impl AsRef<Path>) -> Result<ElfMap, crate::credentials::CredentialError> {
    let bytes = std::fs::read(path.as_ref())?;
    Ok(map_elf(&bytes))
}

pub fn map_elf(data: &[u8]) -> ElfMap {
    let mut mapped = ElfMap {
        file_size: data.len(),
        ..ElfMap::default()
    };
    if data.len() < 64 || !data.starts_with(ELF_MAGIC) {
        mapped.notes.push("not an ELF64 image".into());
        return mapped;
    }
    if data[4] != 2 || data[5] != 1 {
        mapped.notes.push("need ELF64 little-endian".into());
        return mapped;
    }
    let phoff = u64_le(data, 32);
    let shoff = u64_le(data, 40);
    let phentsize = u16_le(data, 54) as usize;
    let phnum = u16_le(data, 56) as usize;
    let shentsize = u16_le(data, 58) as usize;
    let shnum = u16_le(data, 60) as usize;
    let shstrndx = u16_le(data, 62) as usize;

    for i in 0..phnum {
        let off = phoff as usize + i * phentsize;
        if off + 56 > data.len() {
            break;
        }
        let p_type = u32_le(data, off);
        if p_type != PT_LOAD {
            continue;
        }
        let vaddr = u64_le(data, off + 16);
        let filesz = u64_le(data, off + 32);
        let memsz = u64_le(data, off + 40);
        let file_off = u64_le(data, off + 8) as usize;
        let slice_len = (filesz as usize).min(data.len().saturating_sub(file_off));
        let entropy = shannon_entropy(&data[file_off..file_off + slice_len]);
        let load = PtLoad {
            index: i,
            vaddr,
            memsz,
            filesz,
            entropy,
        };
        mapped.notes.push(load.note());
        mapped.pt_load.push(load);
    }

    let shstr = section_bytes(data, shoff, shentsize, shnum, shstrndx);
    for i in 0..shnum {
        let off = shoff as usize + i * shentsize;
        if off + 64 > data.len() {
            break;
        }
        let name_off = u32_le(data, off) as usize;
        let name = cstr_at(shstr, name_off);
        if !name.is_empty() {
            mapped.sections.push(name.clone());
            if name.contains("vmp") || name.contains("VMP") {
                mapped.notes.push(format!("vmp section {name}"));
            }
        }
        let sh_type = u32_le(data, off + 4);
        if sh_type == SHT_DYNSYM || sh_type == SHT_SYMTAB {
            let link = u32_le(data, off + 40) as usize;
            let strtab = section_bytes(data, shoff, shentsize, shnum, link);
            let syms = section_bytes(data, shoff, shentsize, shnum, i);
            mapped
                .network_exports
                .extend(dynsym_network_names(syms, strtab));
        }
    }
    mapped.network_exports.sort();
    mapped.network_exports.dedup();
    mapped.notes.push(format!(
        "exports={}/~108 bambu_network_* file_size={}",
        mapped.network_exports.len(),
        mapped.file_size
    ));
    mapped
}

pub fn map_notes(path: &Path) -> Vec<String> {
    match map_elf_file(path) {
        Ok(m) => {
            let mut n = vec![format!("elf map {}", path.display())];
            n.extend(m.notes);
            n
        }
        Err(err) => vec![format!("elf map {}: {err}", path.display())],
    }
}

pub fn parse_proc_maps(maps: &str) -> Vec<ProcMap> {
    let mut out = Vec::new();
    for line in maps.lines() {
        let mut parts = line.split_whitespace();
        let Some(range) = parts.next() else {
            continue;
        };
        let Some(perms) = parts.next() else {
            continue;
        };
        let pathname = parts.nth(3).unwrap_or("").to_string();
        let Some((start, end)) = range.split_once('-') else {
            continue;
        };
        let (Ok(start), Ok(end)) = (u64::from_str_radix(start, 16), u64::from_str_radix(end, 16))
        else {
            continue;
        };
        if end <= start {
            continue;
        }
        out.push(ProcMap {
            start,
            end,
            perms: perms.to_string(),
            pathname,
        });
    }
    out
}

#[allow(dead_code)]
pub fn plugin_readable_maps(maps: &[ProcMap]) -> Vec<ProcMap> {
    maps.iter()
        .filter(|m| {
            m.perms.contains('r')
                && (m.pathname.contains("bambu_networking") || m.pathname.contains("bambunetwork"))
        })
        .cloned()
        .collect()
}

pub fn harvest_maps(maps: &[ProcMap]) -> Vec<ProcMap> {
    maps.iter()
        .filter(|m| {
            let plugin =
                m.pathname.contains("bambu_networking") || m.pathname.contains("bambunetwork");
            if plugin {
                return m.perms.contains('r');
            }
            m.perms.starts_with("rw")
                && (m.pathname.is_empty()
                    || m.pathname == "[heap]"
                    || m.pathname == "[stack]"
                    || m.pathname.starts_with("[anon")
                    || m.pathname.contains("memfd")
                    || m.pathname.contains("libcrypto")
                    || m.pathname.contains("libssl"))
        })
        .filter(|m| m.end.saturating_sub(m.start) <= 64 * 1024 * 1024)
        .cloned()
        .collect()
}

/// Concatenate mappings in address order into one linear buffer (gaps zeroed).
#[allow(dead_code)]
pub fn reconstruct_linear(
    maps: &[ProcMap],
    mut read: impl FnMut(u64, u64) -> Option<Vec<u8>>,
) -> Vec<u8> {
    if maps.is_empty() {
        return Vec::new();
    }
    let start = maps.iter().map(|m| m.start).min().unwrap();
    let end = maps.iter().map(|m| m.end).max().unwrap();
    if end <= start || end - start > 96 * 1024 * 1024 {
        return Vec::new();
    }
    let mut image = vec![0u8; (end - start) as usize];
    for m in maps {
        if let Some(chunk) = read(m.start, m.end) {
            let off = (m.start - start) as usize;
            let n = chunk.len().min(image.len().saturating_sub(off));
            image[off..off + n].copy_from_slice(&chunk[..n]);
        }
    }
    image
}

pub fn scan_image(data: &[u8]) -> SlicerCredentials {
    let mut creds = extract_pems_plain(data);
    merge_creds(&mut creds, scan_der(data));
    if creds.key_pem.is_none() {
        if let Some(cert) = &creds.cert_pem {
            if let Some(key) = scan_key_matching(data, cert) {
                creds.key_pem = Some(key);
            }
        }
    }
    validate_scanned(&creds)
}

/// Find a PKCS#1/PKCS#8 RSA private key in `data` whose modulus matches `cert_pem`.
pub fn scan_key_matching(data: &[u8], cert_pem: &str) -> Option<String> {
    let want = public_key_from_cert_pem(cert_pem).ok()?;
    if let Some(key) = key_pem_if_matches(scan_der(data).key_pem, &want) {
        return Some(key);
    }
    let n_be = want.n().to_bytes_be();
    let n_le = want.n().to_bytes_le();
    let mut from = 0;
    while let Some(rel) = find_bytes(&data[from..], &n_be) {
        let at = from + rel;
        let start = at.saturating_sub(64);
        let end = (at + n_be.len() + 64).min(data.len());
        if let Some(key) = key_pem_if_matches(scan_der(&data[start..end]).key_pem, &want) {
            return Some(key);
        }
        from = at + 1;
    }
    let mut le_hits = Vec::new();
    from = 0;
    while let Some(rel) = find_bytes(&data[from..], &n_le) {
        let at = from + rel;
        le_hits.push(at);
        let start = at.saturating_sub(64);
        let end = (at + n_le.len() + 64).min(data.len());
        if let Some(key) = key_pem_if_matches(scan_der(&data[start..end]).key_pem, &want) {
            return Some(key);
        }
        from = at + 1;
    }
    // n_be lives in the public cert; OpenSSL BIGNUM limbs are little-endian.
    // Only hunt primes when we saw n_le (or a tiny test blob).
    if le_hits.is_empty() {
        if data.len() <= 64 * 1024 {
            return scan_key_from_prime_limbs(data, &want);
        }
        return None;
    }
    if data.len() <= 4 * 1024 * 1024 {
        return scan_key_from_prime_limbs(data, &want);
    }
    for at in le_hits {
        let start = at.saturating_sub(256 * 1024);
        let end = (at + 256 * 1024).min(data.len());
        if let Some(key) = scan_key_from_prime_limbs(&data[start..end], &want) {
            return Some(key);
        }
    }
    None
}

fn key_pem_if_matches(pem: Option<String>, want: &rsa::RsaPublicKey) -> Option<String> {
    let pem = pem?;
    let key = load_private_key(&pem).ok()?;
    (key.n() == want.n()).then_some(pem)
}

/// OpenSSL keeps RSA primes as little-endian BIGNUM limbs after the PEM is dropped.
fn scan_key_from_prime_limbs(data: &[u8], want: &rsa::RsaPublicKey) -> Option<String> {
    let n = want.n();
    let e = want.e();
    let half_bits = n.bits().saturating_add(1) / 2;
    let p_len = ((half_bits + 7) / 8) as usize;
    if p_len < 32 || data.len() < p_len {
        return None;
    }
    for len in [p_len, p_len.saturating_add(1), p_len.saturating_sub(1)] {
        if len < 32 || len > data.len() {
            continue;
        }
        if let Some(key) = try_prime_windows(data, n, e, len, true) {
            return Some(key);
        }
        if let Some(key) = try_prime_windows(data, n, e, len, false) {
            return Some(key);
        }
    }
    None
}

fn try_prime_windows(
    data: &[u8],
    n: &BigUint,
    e: &BigUint,
    p_len: usize,
    little: bool,
) -> Option<String> {
    let min_bits = (n.bits() / 2).saturating_sub(8).max(32);
    let mut i = 0;
    while i + p_len <= data.len() {
        let raw = &data[i..i + p_len];
        let odd = if little {
            raw[0] & 1 == 1
        } else {
            raw[p_len - 1] & 1 == 1
        };
        if !odd {
            i += 8;
            continue;
        }
        let p = if little {
            BigUint::from_bytes_le(raw)
        } else {
            BigUint::from_bytes_be(raw)
        };
        if p.bits() >= min_bits
            && &p > &BigUint::from(2u32)
            && p != *n
            && n % &p == BigUint::from(0u32)
        {
            let q = n / &p;
            if &p * &q == *n {
                if let Ok(key) = RsaPrivateKey::from_p_q(p, q, e.clone()) {
                    if key.n() == n {
                        return key
                            .to_pkcs8_pem(LineEnding::LF)
                            .ok()
                            .map(|pem| pem.to_string());
                    }
                }
            }
        }
        i += 8;
    }
    None
}

pub(crate) fn find_bytes(hay: &[u8], needle: &[u8]) -> Option<usize> {
    hay.windows(needle.len()).position(|w| w == needle)
}

/// Try AES-GCM layouts against `get_app_cert` JSON `"key"` blobs using captured RAND.
pub fn try_decrypt_app_key(dump: &[u8], rand_records: &[u8]) -> Option<String> {
    let keys = parse_rand_keys(rand_records);
    if keys.is_empty() {
        return None;
    }
    let blobs = json_b64_fields(dump, br#""key":""#);
    for blob in blobs {
        if blob.len() < 48 {
            continue;
        }
        for k in &keys {
            if k.len() != 32 {
                continue;
            }
            if let Some(pem) = decrypt_key_blob(k, &blob) {
                if load_private_key(&pem).is_ok() {
                    return Some(pem);
                }
            }
        }
    }
    None
}

fn parse_rand_keys(records: &[u8]) -> Vec<Vec<u8>> {
    let mut out = Vec::new();
    let mut i = 0;
    while i < records.len() {
        let n = records[i] as usize;
        i += 1;
        if n == 0 || i + n > records.len() {
            break;
        }
        if n == 32 {
            out.push(records[i..i + n].to_vec());
        }
        i += n;
    }
    out
}

pub(crate) fn json_b64_fields(data: &[u8], prefix: &[u8]) -> Vec<Vec<u8>> {
    let mut out = Vec::new();
    let mut from = 0;
    while let Some(rel) = find_bytes(&data[from..], prefix) {
        let start = from + rel + prefix.len();
        let rest = &data[start..];
        let n = rest
            .iter()
            .take_while(|b| {
                matches!(
                    **b,
                    b'A'..=b'Z' | b'a'..=b'z' | b'0'..=b'9' | b'+' | b'/' | b'=' | b'-' | b'_'
                )
            })
            .count();
        if n >= 64 {
            let slice = &rest[..n];
            let raw = base64::engine::general_purpose::STANDARD
                .decode(slice)
                .or_else(|_| base64::engine::general_purpose::URL_SAFE.decode(slice))
                .or_else(|_| base64::engine::general_purpose::URL_SAFE_NO_PAD.decode(slice));
            if let Ok(raw) = raw {
                if !out.iter().any(|b| b == &raw) {
                    out.push(raw);
                }
            }
        }
        from = start + n.max(1);
    }
    out
}

fn decrypt_key_blob(k: &[u8], blob: &[u8]) -> Option<String> {
    if blob.len() < 48 {
        return None;
    }
    let derived = hkdf_app_key(k, blob.get(4..16).unwrap_or(&[]));
    let keys = [k, derived.as_slice()];
    let n = blob.len();
    let layouts: [(&[u8], &[u8], &[u8]); 3] = [
        (&blob[..12], &blob[12..n - 16], &blob[n - 16..]),
        (&blob[4..16], &blob[32..], &blob[16..32]),
        (&blob[4..16], &blob[16..n - 16], &blob[n - 16..]),
    ];
    for key in keys {
        for (nonce, ct, tag) in layouts {
            if nonce.len() != 12 || ct.is_empty() || tag.len() != 16 {
                continue;
            }
            let mut payload = Vec::with_capacity(ct.len() + 16);
            payload.extend_from_slice(ct);
            payload.extend_from_slice(tag);
            if let Some(pt) = aes_gcm_decrypt(key, nonce, &payload) {
                if let Some(pem) = pem_from_bytes(&pt) {
                    return Some(pem);
                }
            }
        }
    }
    None
}

fn hkdf_app_key(ikm: &[u8], salt: &[u8]) -> [u8; 32] {
    let hk = Hkdf::<Sha256>::new(Some(salt), ikm);
    let mut okm = [0u8; 32];
    let _ = hk.expand(b"bambu_app_key_v1", &mut okm);
    okm
}

fn aes_gcm_decrypt(key: &[u8], nonce: &[u8], payload: &[u8]) -> Option<Vec<u8>> {
    if key.len() != 32 || nonce.len() != 12 {
        return None;
    }
    let cipher = Aes256Gcm::new_from_slice(key).ok()?;
    let nonce = Nonce::from_slice(nonce);
    cipher.decrypt(nonce, payload).ok()
}

fn pem_from_bytes(pt: &[u8]) -> Option<String> {
    let creds = extract_pems_plain(pt);
    creds.key_pem.or_else(|| {
        let text = String::from_utf8_lossy(pt);
        if text.contains("BEGIN PRIVATE") {
            Some(text.into_owned())
        } else {
            None
        }
    })
}

fn validate_scanned(creds: &SlicerCredentials) -> SlicerCredentials {
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

fn scan_der(data: &[u8]) -> SlicerCredentials {
    let mut creds = SlicerCredentials::default();
    let mut i = 0;
    while i + 4 < data.len() {
        if data[i] != 0x30 || data[i + 1] != 0x82 {
            i += 1;
            continue;
        }
        let len = u16::from_be_bytes([data[i + 2], data[i + 3]]) as usize;
        if !(200..=8000).contains(&len) {
            i += 1;
            continue;
        }
        let end = i + 4 + len;
        if end > data.len() {
            break;
        }
        let der = &data[i..end];
        if creds.cert_pem.is_none() && X509Certificate::from_der(der).is_ok() {
            creds.cert_pem = Some(der_pem("CERTIFICATE", der));
        } else if creds.key_pem.is_none() && RsaPrivateKey::from_pkcs8_der(der).is_ok() {
            creds.key_pem = Some(der_pem("PRIVATE KEY", der));
        } else if creds.key_pem.is_none() && RsaPrivateKey::from_pkcs1_der(der).is_ok() {
            creds.key_pem = Some(der_pem("RSA PRIVATE KEY", der));
        } else if creds.crl_pem.is_none() && CertificateRevocationList::from_der(der).is_ok() {
            creds.crl_pem = Some(der_pem("X509 CRL", der));
        }
        i += 1;
        if creds.has_cert_and_key() && creds.crl_pem.is_some() {
            break;
        }
    }
    creds
}

fn der_pem(kind: &str, der: &[u8]) -> String {
    let b64 = base64::engine::general_purpose::STANDARD.encode(der);
    let mut out = format!("-----BEGIN {kind}-----\n");
    for chunk in b64.as_bytes().chunks(64) {
        out.push_str(std::str::from_utf8(chunk).unwrap_or(""));
        out.push('\n');
    }
    out.push_str("-----END ");
    out.push_str(kind);
    out.push_str("-----\n");
    out
}

fn shannon_entropy(data: &[u8]) -> f64 {
    if data.is_empty() {
        return 0.0;
    }
    let mut counts = [0u32; 256];
    for b in data {
        counts[*b as usize] += 1;
    }
    let n = data.len() as f64;
    counts
        .iter()
        .filter(|c| **c > 0)
        .map(|c| {
            let p = f64::from(*c) / n;
            -p * p.log2()
        })
        .sum()
}

fn section_bytes(data: &[u8], shoff: u64, shentsize: usize, shnum: usize, idx: usize) -> &[u8] {
    if idx >= shnum {
        return &[];
    }
    let off = shoff as usize + idx * shentsize;
    if off + 64 > data.len() {
        return &[];
    }
    let offset = u64_le(data, off + 24) as usize;
    let size = u64_le(data, off + 32) as usize;
    data.get(offset..offset.saturating_add(size)).unwrap_or(&[])
}

fn dynsym_network_names(syms: &[u8], strtab: &[u8]) -> Vec<String> {
    let mut out = Vec::new();
    let mut i = 0;
    while i + 24 <= syms.len() {
        let st_name = u32_le(syms, i) as usize;
        let st_info = syms[i + 4];
        let ty = st_info & 0xf;
        i += 24;
        if ty != STT_FUNC && ty != STT_OBJECT && ty != 0 {
            continue;
        }
        let name = cstr_at(strtab, st_name);
        if name.starts_with("bambu_network_") {
            out.push(name);
        }
    }
    out
}

fn cstr_at(data: &[u8], off: usize) -> String {
    if off >= data.len() {
        return String::new();
    }
    let end = data[off..]
        .iter()
        .position(|b| *b == 0)
        .map(|p| off + p)
        .unwrap_or(data.len());
    String::from_utf8_lossy(&data[off..end]).into_owned()
}

fn u16_le(data: &[u8], off: usize) -> u16 {
    u16::from_le_bytes(data[off..off + 2].try_into().unwrap_or([0, 0]))
}

fn u32_le(data: &[u8], off: usize) -> u32 {
    u32::from_le_bytes(data[off..off + 4].try_into().unwrap_or([0, 0, 0, 0]))
}

fn u64_le(data: &[u8], off: usize) -> u64 {
    u64::from_le_bytes(
        data[off..off + 8]
            .try_into()
            .unwrap_or([0, 0, 0, 0, 0, 0, 0, 0]),
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    const SHT_PROGBITS: u32 = 1;
    const SHT_STRTAB: u32 = 3;

    fn put_u16(buf: &mut [u8], off: usize, v: u16) {
        buf[off..off + 2].copy_from_slice(&v.to_le_bytes());
    }
    fn put_u32(buf: &mut [u8], off: usize, v: u32) {
        buf[off..off + 4].copy_from_slice(&v.to_le_bytes());
    }
    fn put_u64(buf: &mut [u8], off: usize, v: u64) {
        buf[off..off + 8].copy_from_slice(&v.to_le_bytes());
    }

    fn synthetic_elf() -> Vec<u8> {
        let mut text = vec![0u8; 256];
        for (i, b) in text.iter_mut().enumerate() {
            *b = (i.wrapping_mul(73) ^ 0xA5) as u8;
        }
        let dynstr = b"\0bambu_network_get_version\0".to_vec();
        let mut dynsym = vec![0u8; 48];
        put_u32(&mut dynsym, 24, 1);
        dynsym[28] = STT_FUNC;
        let shstr = b"\0.text\0.dynstr\0.dynsym\0.shstrtab\0.vmp0\0".to_vec();
        let ehdr = 64usize;
        let phdr = 56usize;
        let text_off = ehdr + phdr;
        let dynstr_off = text_off + text.len();
        let dynsym_off = dynstr_off + dynstr.len();
        let shstr_off = dynsym_off + dynsym.len();
        let vmp_off = shstr_off + shstr.len();
        let mut vmp = vec![0u8; 64];
        for (i, b) in vmp.iter_mut().enumerate() {
            *b = (i as u8).wrapping_mul(19).wrapping_add(7);
        }
        let shoff = vmp_off + vmp.len();
        let shnum = 6u16;
        let mut file = vec![0u8; shoff + 64 * shnum as usize];
        file[0..4].copy_from_slice(ELF_MAGIC);
        file[4] = 2;
        file[5] = 1;
        file[6] = 1;
        put_u16(&mut file, 16, 3);
        put_u16(&mut file, 18, 62);
        put_u32(&mut file, 20, 1);
        put_u64(&mut file, 32, 64);
        put_u64(&mut file, 40, shoff as u64);
        put_u16(&mut file, 52, 64);
        put_u16(&mut file, 54, 56);
        put_u16(&mut file, 56, 1);
        put_u16(&mut file, 58, 64);
        put_u16(&mut file, 60, shnum);
        put_u16(&mut file, 62, 4);
        put_u32(&mut file, 64, PT_LOAD);
        put_u64(&mut file, 64 + 8, 0);
        put_u64(&mut file, 64 + 16, 0);
        put_u64(&mut file, 64 + 32, shoff as u64);
        put_u64(&mut file, 64 + 40, shoff as u64);
        file[text_off..text_off + text.len()].copy_from_slice(&text);
        file[dynstr_off..dynstr_off + dynstr.len()].copy_from_slice(&dynstr);
        file[dynsym_off..dynsym_off + dynsym.len()].copy_from_slice(&dynsym);
        file[shstr_off..shstr_off + shstr.len()].copy_from_slice(&shstr);
        file[vmp_off..vmp_off + vmp.len()].copy_from_slice(&vmp);
        let mut write_shdr = |i: usize, name: u32, ty: u32, offset: u64, size: u64, link: u32| {
            let off = shoff + i * 64;
            put_u32(&mut file, off, name);
            put_u32(&mut file, off + 4, ty);
            put_u64(&mut file, off + 24, offset);
            put_u64(&mut file, off + 32, size);
            put_u32(&mut file, off + 40, link);
        };
        write_shdr(0, 0, 0, 0, 0, 0);
        write_shdr(1, 1, SHT_PROGBITS, text_off as u64, text.len() as u64, 0);
        write_shdr(2, 7, SHT_STRTAB, dynstr_off as u64, dynstr.len() as u64, 0);
        write_shdr(3, 15, SHT_DYNSYM, dynsym_off as u64, dynsym.len() as u64, 2);
        write_shdr(4, 23, SHT_STRTAB, shstr_off as u64, shstr.len() as u64, 0);
        write_shdr(5, 33, SHT_PROGBITS, vmp_off as u64, vmp.len() as u64, 0);
        file
    }

    #[test]
    fn maps_synthetic_elf_exports_and_vmp_section() {
        let elf = synthetic_elf();
        let mapped = map_elf(&elf);
        assert!(!mapped.pt_load.is_empty());
        assert!(mapped.pt_load[0].entropy > 1.0);
        assert!(mapped.sections.iter().any(|s| s == ".vmp0"));
        assert!(
            mapped
                .network_exports
                .iter()
                .any(|s| s == "bambu_network_get_version"),
            "{:?}",
            mapped.network_exports
        );
        assert!(mapped.notes.iter().any(|n| n.starts_with("PT_LOAD[0]")));
    }

    #[test]
    fn reconstruct_linear_from_fake_maps() {
        let maps = parse_proc_maps(
            "7f100000-7f100010 r-xp 00000000 00:00 1 /tmp/libbambu_networking.so\n\
             7f100010-7f100020 r--p 00000010 00:00 1 /tmp/libbambu_networking.so\n",
        );
        let plugin = plugin_readable_maps(&maps);
        assert_eq!(plugin.len(), 2);
        let image = reconstruct_linear(&plugin, |start, end| {
            Some(vec![((start >> 4) & 0xff) as u8; (end - start) as usize])
        });
        assert_eq!(image.len(), 0x20);
        assert_eq!(image[0], 0x00);
        assert_eq!(image[0x10], 0x01);
    }

    #[test]
    fn scan_image_finds_pem_fixture() {
        let cert = include_str!("../tests/fixtures/test_slicer_cert.pem");
        let key = include_str!("../tests/fixtures/test_slicer_key.pem");
        let mut blob = vec![0u8; 32];
        blob.extend_from_slice(cert.as_bytes());
        blob.extend_from_slice(b"\n");
        blob.extend_from_slice(key.as_bytes());
        let creds = scan_image(&blob);
        assert!(creds.has_cert_and_key());
    }

    #[test]
    fn scan_image_finds_der_cert() {
        let cert = include_str!("../tests/fixtures/test_slicer_cert.pem");
        let (_, pem) = x509_parser::pem::parse_x509_pem(cert.as_bytes()).unwrap();
        let mut blob = vec![0u8; 16];
        blob.extend_from_slice(&pem.contents);
        let creds = scan_image(&blob);
        assert!(
            creds
                .cert_pem
                .as_ref()
                .is_some_and(|p| p.contains("BEGIN CERTIFICATE")),
            "DER cert scan missed fixture"
        );
    }

    #[test]
    fn harvest_maps_keeps_rx_plugin_and_heap() {
        let maps = parse_proc_maps(
            "7f000000-7f001000 r-xp 00000000 00:00 0 /usr/lib/libc.so.6\n\
             7f100000-7f180000 rw-p 00000000 00:00 0 [heap]\n\
             7f1a0000-7f1b0000 r-xp 00000000 00:00 1 /tmp/libbambu_networking.so\n\
             7f1b0000-7f1c0000 rw-p 00000010 00:00 1 /tmp/libbambu_networking.so\n\
             7f300000-7f400000 rw-p 00000000 00:00 0\n",
        );
        let harvested = harvest_maps(&maps);
        assert_eq!(harvested.len(), 4, "{harvested:?}");
        assert!(plugin_readable_maps(&maps)
            .iter()
            .any(|m| m.perms.contains("r-x")));
    }

    #[test]
    fn scan_key_matching_finds_pkcs8_der() {
        let cert = include_str!("../tests/fixtures/test_slicer_cert.pem");
        let key_pem = include_str!("../tests/fixtures/test_slicer_key.pem");
        let key = RsaPrivateKey::from_pkcs8_pem(key_pem).unwrap();
        let der = key.to_pkcs8_der().unwrap();
        let mut blob = vec![0u8; 64];
        blob.extend_from_slice(der.as_bytes());
        let found = scan_key_matching(&blob, cert).expect("DER key matching cert");
        assert!(load_private_key(&found).is_ok());
    }

    #[test]
    fn scan_key_matching_finds_openssl_le_prime() {
        use rsa::traits::PrivateKeyParts;
        let cert = include_str!("../tests/fixtures/test_slicer_cert.pem");
        let key_pem = include_str!("../tests/fixtures/test_slicer_key.pem");
        let key = RsaPrivateKey::from_pkcs8_pem(key_pem).unwrap();
        let p = &key.primes()[0];
        let mut blob = vec![0u8; 24];
        blob.extend_from_slice(&p.to_bytes_le());
        blob.extend_from_slice(&[0u8; 16]);
        let found = scan_key_matching(&blob, cert).expect("LE prime limbs matching cert");
        let recovered = load_private_key(&found).expect("recovered key");
        assert_eq!(recovered.n(), key.n());
    }

    #[test]
    fn decrypt_app_key_roundtrip_nonce_ct_tag() {
        let key_pem = include_str!("../tests/fixtures/test_slicer_key.pem");
        let session = [0x11u8; 32];
        let nonce = [0x22u8; 12];
        let cipher = Aes256Gcm::new_from_slice(&session).unwrap();
        let ct = cipher
            .encrypt(Nonce::from_slice(&nonce), key_pem.as_bytes())
            .unwrap();
        let mut blob = nonce.to_vec();
        blob.extend_from_slice(&ct);
        let dump = format!(
            r#"{{"key":"{}"}}"#,
            base64::engine::general_purpose::STANDARD.encode(&blob)
        );
        let mut rands = vec![32u8];
        rands.extend_from_slice(&session);
        let found = try_decrypt_app_key(dump.as_bytes(), &rands).expect("decrypt fixture key");
        assert!(load_private_key(&found).is_ok());
    }
}
