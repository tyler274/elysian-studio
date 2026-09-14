//! Recover `client_auth_secret` + `server_wrap_key` from an unpacked plugin dump.
//!
//! ClusterM §10.2: both are embedded in the stock plugin (not login tokens). The
//! helper dump often has the live cert URL; AES-GCM with a nearby 32-byte
//! `session_key` opens `{enc_secret}` back to the ASCII secret.

use aes_gcm::aead::{Aead, KeyInit};
use aes_gcm::{Aes256Gcm, Nonce};
use rsa::pkcs1::DecodeRsaPublicKey;
use rsa::pkcs8::{DecodePublicKey, EncodePublicKey, LineEnding};
use rsa::traits::PublicKeyParts;
use rsa::RsaPublicKey;

use crate::credentials::SlicerCredentials;
use crate::extract_appcert::b64decode_any;
use crate::extract_elf::find_bytes;
use crate::signing::public_key_from_cert_pem;

/// Scan dump (+ optional getrandom records) for bootstrap files.
pub fn harvest_bootstrap(data: &[u8], rand_records: &[u8]) -> Option<SlicerCredentials> {
    let secret = find_plaintext_secret(data).or_else(|| recover_secret_from_cert_url(data, rand_records));
    let wrap = find_wrap_pem(data);
    if secret.is_none() && wrap.is_none() {
        return None;
    }
    Some(SlicerCredentials {
        client_auth_secret: secret,
        server_wrap_pem: wrap,
        ..SlicerCredentials::default()
    })
}

fn looks_like_client_auth_secret(s: &[u8]) -> bool {
    (27..=64).contains(&s.len())
        && s.starts_with(b"GLOF")
        && s.iter().all(|b| b.is_ascii_graphic() && *b != b'/')
        && s.contains(&b'-')
}

fn find_plaintext_secret(data: &[u8]) -> Option<Vec<u8>> {
    if let Some(s) = scan_glof_secret(data) {
        return Some(s);
    }
    if data.len() > 4 * 1024 * 1024 {
        return None;
    }
    let needle = b"GLOF3813734089-";
    for k in 1u8..=255 {
        let xored: Vec<u8> = needle.iter().map(|b| b ^ k).collect();
        let mut from = 0;
        while let Some(rel) = find_bytes(&data[from..], &xored) {
            let at = from + rel;
            let take = (at + 64).min(data.len());
            let decoded: Vec<u8> = data[at..take].iter().map(|b| b ^ k).collect();
            let n = decoded
                .iter()
                .take_while(|b| b.is_ascii_graphic() && **b != b'/')
                .count();
            if looks_like_client_auth_secret(&decoded[..n]) && n >= 40 {
                return Some(decoded[..n].to_vec());
            }
            from = at + 1;
        }
    }
    None
}

fn scan_glof_secret(data: &[u8]) -> Option<Vec<u8>> {
    let mut from = 0;
    while let Some(rel) = find_bytes(&data[from..], b"GLOF") {
        let at = from + rel;
        let n = data[at..]
            .iter()
            .take_while(|b| b.is_ascii_graphic() && **b != b'/')
            .count();
        if looks_like_client_auth_secret(&data[at..at + n]) && n >= 40 {
            return Some(data[at..at + n].to_vec());
        }
        from = at + 1;
    }
    None
}

fn recover_secret_from_cert_url(data: &[u8], rand_records: &[u8]) -> Option<Vec<u8>> {
    let mut keys = parse_rand32(rand_records);
    for (enc, _, at) in cert_url_blobs(data) {
        if enc.len() < 12 + 16 {
            continue;
        }
        let iv = &enc[..12];
        let ct_tag = &enc[12..];
        let lo = at.saturating_sub(65536);
        let hi = (at + 65536).min(data.len());
        let mut i = lo;
        while i + 32 <= hi {
            keys.push(data[i..i + 32].to_vec());
            i += 4;
        }
        let mut from = 0;
        let mut iv_hits = 0usize;
        while let Some(rel) = find_bytes(&data[from..], iv) {
            let at_iv = from + rel;
            iv_hits += 1;
            let lo = at_iv.saturating_sub(256);
            let hi = (at_iv + 12 + 256).min(data.len());
            let mut j = lo;
            while j + 32 <= hi {
                if j + 32 <= at_iv || j >= at_iv + 12 {
                    keys.push(data[j..j + 32].to_vec());
                }
                j += 1;
            }
            from = at_iv + 1;
            if iv_hits >= 32 {
                break;
            }
        }
        if keys.len() > 50_000 {
            keys.truncate(50_000);
        }
        keys_near_glof(data, &mut keys);
        if keys.len() > 120_000 {
            keys.truncate(120_000);
        }
        for key in &keys {
            if key.len() != 32 {
                continue;
            }
            if let Some(pt) = aes_gcm_open(key, iv, ct_tag) {
                if looks_like_client_auth_secret(&pt) {
                    return Some(pt);
                }
            }
        }
    }
    None
}

fn cert_url_blobs(data: &[u8]) -> Vec<(Vec<u8>, Vec<u8>, usize)> {
    let mut out = Vec::new();
    let mut from = 0;
    let prefix = b"/applications/";
    while let Some(rel) = find_bytes(&data[from..], prefix) {
        let url_at = from + rel;
        let start = url_at + prefix.len();
        let rest = &data[start..];
        let enc_n = rest
            .iter()
            .take_while(|b| {
                matches!(
                    **b,
                    b'A'..=b'Z' | b'a'..=b'z' | b'0'..=b'9' | b'-' | b'_' | b'='
                )
            })
            .count();
        let Some(enc) = b64decode_any(std::str::from_utf8(&rest[..enc_n]).unwrap_or("")) else {
            from = start + 1;
            continue;
        };
        let after = &rest[enc_n..];
        let Some(wrap) = after.strip_prefix(b"/cert?aes256=") else {
            from = start + enc_n.max(1);
            continue;
        };
        let wrap_n = wrap
            .iter()
            .take_while(|b| {
                matches!(
                    **b,
                    b'A'..=b'Z' | b'a'..=b'z' | b'0'..=b'9' | b'-' | b'_' | b'='
                )
            })
            .count();
        if let Some(aes) = b64decode_any(std::str::from_utf8(&wrap[..wrap_n]).unwrap_or("")) {
            if !out.iter().any(|(e, _, _)| e == &enc) {
                out.push((enc, aes, url_at));
            }
        }
        from = start + enc_n.max(1);
    }
    out
}

fn aes_gcm_open(key: &[u8], iv: &[u8], ct_tag: &[u8]) -> Option<Vec<u8>> {
    if key.len() != 32 || iv.len() != 12 || ct_tag.len() < 16 {
        return None;
    }
    let cipher = Aes256Gcm::new_from_slice(key).ok()?;
    let nonce = Nonce::from_slice(iv);
    cipher.decrypt(nonce, ct_tag).ok()
}

fn parse_rand32(records: &[u8]) -> Vec<Vec<u8>> {
    let mut out = Vec::new();
    let mut sixteens: Vec<Vec<u8>> = Vec::new();
    let mut i = 0;
    while i < records.len() {
        let n = records[i] as usize;
        i += 1;
        if n == 0 || i + n > records.len() {
            break;
        }
        if n == 32 {
            out.push(records[i..i + n].to_vec());
        } else if n == 16 {
            sixteens.push(records[i..i + n].to_vec());
        } else if n > 32 {
            for j in 0..=n - 32 {
                out.push(records[i + j..i + j + 32].to_vec());
            }
        }
        i += n;
    }
    for a in 0..sixteens.len() {
        for b in 0..sixteens.len() {
            if a == b {
                continue;
            }
            let mut k = sixteens[a].clone();
            k.extend_from_slice(&sixteens[b]);
            out.push(k);
        }
    }
    out
}

fn keys_near_glof(data: &[u8], keys: &mut Vec<Vec<u8>>) {
    let mut from = 0;
    let mut hits = 0usize;
    while let Some(rel) = find_bytes(&data[from..], b"GLOF") {
        let at = from + rel;
        hits += 1;
        let lo = at.saturating_sub(4096);
        let hi = (at + 4096).min(data.len());
        let mut i = lo;
        while i + 32 <= hi {
            keys.push(data[i..i + 32].to_vec());
            i += 4;
        }
        from = at + 1;
        if hits >= 16 {
            break;
        }
    }
}

/// PEM or SPKI/PKCS#1 DER captured by the in-process hook.
pub(crate) fn wrap_bytes_to_pem(raw: &[u8]) -> Option<String> {
    let trimmed = raw.trim_ascii();
    if let Ok(text) = std::str::from_utf8(trimmed) {
        if text.contains("BEGIN PUBLIC") {
            if RsaPublicKey::from_public_key_pem(text).is_ok()
                || RsaPublicKey::from_pkcs1_pem(text).is_ok()
            {
                return Some(text.trim().to_string() + "\n");
            }
        }
    }
    if let Ok(key) = RsaPublicKey::from_public_key_der(trimmed) {
        if key.size() == 256 {
            return key.to_public_key_pem(LineEnding::LF).ok();
        }
    }
    if let Ok(key) = RsaPublicKey::from_pkcs1_der(trimmed) {
        if key.size() == 256 {
            return key.to_public_key_pem(LineEnding::LF).ok();
        }
    }
    None
}

fn find_wrap_pem(data: &[u8]) -> Option<String> {
    if let Some(pem) = pem_block_containing(data, "BEGIN PUBLIC KEY") {
        if RsaPublicKey::from_public_key_pem(&pem).is_ok() {
            return Some(pem);
        }
    }
    if let Some(pem) = pem_block_containing(data, "BEGIN RSA PUBLIC KEY") {
        if RsaPublicKey::from_pkcs1_pem(&pem).is_ok() {
            return Some(pem);
        }
    }
    xor_public_key_pem(data).or_else(|| standalone_wrap_key(data))
}

fn xor_public_key_pem(data: &[u8]) -> Option<String> {
    if data.len() > 4 * 1024 * 1024 {
        return None;
    }
    let needle = b"-----BEGIN PUBLIC KEY-----";
    for k in 1u8..=255 {
        let xored: Vec<u8> = needle.iter().map(|b| b ^ k).collect();
        let mut from = 0;
        let mut hits = 0usize;
        while let Some(rel) = find_bytes(&data[from..], &xored) {
            let at = from + rel;
            hits += 1;
            let take = (at + 800).min(data.len());
            let decoded: Vec<u8> = data[at..take].iter().map(|b| b ^ k).collect();
            if let Some(pem) = pem_block_containing(&decoded, "BEGIN PUBLIC KEY") {
                if RsaPublicKey::from_public_key_pem(&pem).is_ok() {
                    return Some(pem);
                }
            }
            from = at + 1;
            if hits >= 8 {
                break;
            }
        }
    }
    None
}

fn pem_block_containing(data: &[u8], label: &str) -> Option<String> {
    let text = String::from_utf8_lossy(data);
    let begin = format!("-----{label}-----");
    let start = text.find(&begin)?;
    let rest = &text[start..];
    let end_tag = "-----END ";
    let end_rel = rest.find(end_tag)?;
    let tail = &rest[end_rel + end_tag.len()..];
    let nl = tail.find("-----")?;
    let end = start + end_rel + end_tag.len() + nl + 5;
    if end > text.len() {
        return None;
    }
    Some(text[start..end].trim().to_string() + "\n")
}

fn standalone_wrap_key(data: &[u8]) -> Option<String> {
    let cert_n = cert_moduli(data);
    let mut extra: Vec<RsaPublicKey> = Vec::new();
    let hdr = [0x30, 0x82, 0x01, 0x22, 0x30, 0x0d, 0x06, 0x09];
    let mut from = 0;
    while let Some(rel) = find_bytes(&data[from..], &hdr) {
        let i = from + rel;
        let len = u16::from_be_bytes([data[i + 2], data[i + 3]]) as usize;
        let end = i + 4 + len;
        from = i + 1;
        if end > data.len() {
            continue;
        }
        let Ok(key) = RsaPublicKey::from_public_key_der(&data[i..end]) else {
            continue;
        };
        if key.size() != 256 || cert_n.iter().any(|n| n == &key.n().to_bytes_be()) {
            continue;
        }
        push_extra(&mut extra, key);
        if extra.len() > 8 {
            return None;
        }
    }
    collect_pkcs1_2048(data, &cert_n, &mut extra)?;
    (extra.len() == 1)
        .then(|| extra[0].to_public_key_pem(LineEnding::LF).ok())
        .flatten()
}

fn collect_pkcs1_2048(
    data: &[u8],
    cert_n: &[Vec<u8>],
    extra: &mut Vec<RsaPublicKey>,
) -> Option<()> {
    let needle = [0x02, 0x82, 0x01, 0x01, 0x00];
    let mut from = 0;
    while let Some(rel) = find_bytes(&data[from..], &needle) {
        let n_hdr = from + rel;
        from = n_hdr + 1;
        let n_at = n_hdr + 5;
        if n_at < 9 || n_at + 256 + 5 > data.len() {
            continue;
        }
        if !data[n_at + 256..].starts_with(&[0x02, 0x03, 0x01, 0x00, 0x01]) {
            continue;
        }
        let seq = n_at - 9;
        if data[seq] != 0x30 {
            continue;
        }
        let end = n_at + 256 + 5;
        let Ok(key) = RsaPublicKey::from_pkcs1_der(&data[seq..end]) else {
            continue;
        };
        if key.size() != 256 || cert_n.iter().any(|n| n == &key.n().to_bytes_be()) {
            continue;
        }
        push_extra(extra, key);
        if extra.len() > 8 {
            return None;
        }
    }
    Some(())
}

fn push_extra(extra: &mut Vec<RsaPublicKey>, key: RsaPublicKey) {
    if !extra.iter().any(|e| e.n() == key.n()) {
        extra.push(key);
    }
}

fn cert_moduli(data: &[u8]) -> Vec<Vec<u8>> {
    let mut out = Vec::new();
    let text = String::from_utf8_lossy(data);
    let mut i = 0;
    while i < text.len() {
        let Some(start) = text[i..].find("-----BEGIN CERTIFICATE-----") else {
            break;
        };
        let start = i + start;
        let Some(end_rel) = text[start..].find("-----END CERTIFICATE-----") else {
            break;
        };
        let end = start + end_rel + "-----END CERTIFICATE-----".len();
        if let Ok(k) = public_key_from_cert_pem(&text[start..end]) {
            let n = k.n().to_bytes_be();
            if !out.contains(&n) {
                out.push(n);
            }
        }
        i = end;
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;
    use aes_gcm::aead::{Aead, KeyInit};
    use aes_gcm::{Aes256Gcm, Nonce};
    use rsa::pkcs8::{EncodePublicKey, LineEnding};
    use rsa::RsaPublicKey;

    #[test]
    fn harvest_plaintext_secret() {
        let secret = b"GLOF3813734089-836763c700001234567890abcdef";
        let mut dump = vec![0u8; 8];
        dump.extend_from_slice(secret);
        dump.push(0);
        let found = harvest_bootstrap(&dump, &[]).expect("plaintext secret");
        assert_eq!(found.client_auth_secret.as_deref(), Some(secret.as_slice()));
    }

    #[test]
    fn harvest_xored_secret() {
        let secret = b"GLOF3813734089-836763c700001234567890abcdef";
        let dump: Vec<u8> = secret.iter().map(|b| b ^ 0x5a).collect();
        let found = harvest_bootstrap(&dump, &[]).expect("xor secret");
        assert_eq!(found.client_auth_secret.as_deref(), Some(secret.as_slice()));
    }

    #[test]
    fn harvest_secret_from_cert_url_and_nearby_key() {
        use base64::Engine;
        let secret = b"GLOF3813734089-836763c700001234567890abcdef";
        let key = [7u8; 32];
        let iv = [9u8; 12];
        let cipher = Aes256Gcm::new_from_slice(&key).unwrap();
        let ct = cipher
            .encrypt(Nonce::from_slice(&iv), secret.as_ref())
            .unwrap();
        let mut enc = iv.to_vec();
        enc.extend_from_slice(&ct);
        let enc_b64 = base64::engine::general_purpose::URL_SAFE.encode(&enc);
        let mut dump = b"/applications/".to_vec();
        dump.extend_from_slice(enc_b64.as_bytes());
        dump.extend_from_slice(b"/cert?aes256=AAAA&ver=1");
        dump.extend_from_slice(&[0u8; 8]);
        dump.extend_from_slice(&key);
        dump.extend_from_slice(&iv);
        let found = harvest_bootstrap(&dump, &[]).expect("url+key");
        assert_eq!(found.client_auth_secret.as_deref(), Some(secret.as_slice()));
    }

    #[test]
    fn harvest_secret_from_rand_records() {
        use base64::Engine;
        let secret = b"GLOF3813734089-836763c700001234567890abcdef";
        let key = [3u8; 32];
        let iv = [1u8; 12];
        let cipher = Aes256Gcm::new_from_slice(&key).unwrap();
        let ct = cipher
            .encrypt(Nonce::from_slice(&iv), secret.as_ref())
            .unwrap();
        let mut enc = iv.to_vec();
        enc.extend_from_slice(&ct);
        let enc_b64 = base64::engine::general_purpose::URL_SAFE.encode(&enc);
        let mut dump = b"/applications/".to_vec();
        dump.extend_from_slice(enc_b64.as_bytes());
        dump.extend_from_slice(b"/cert?aes256=BBBB&ver=1");
        let mut rec = vec![32u8];
        rec.extend_from_slice(&key);
        let found = harvest_bootstrap(&dump, &rec).expect("rand records");
        assert_eq!(found.client_auth_secret.as_deref(), Some(secret.as_slice()));
    }

    #[test]
    fn wrap_der_roundtrip_to_pem() {
        let key = crate::signing::load_private_key(include_str!(
            "../tests/fixtures/test_slicer_key.pem"
        ))
        .unwrap();
        let pubk = RsaPublicKey::from(&key);
        let der = rsa::pkcs8::EncodePublicKey::to_public_key_der(&pubk)
            .unwrap()
            .to_vec();
        let pem = wrap_bytes_to_pem(&der).expect("der wrap");
        assert!(pem.contains("BEGIN PUBLIC KEY"));
    }

    #[test]
    fn harvest_wrap_pem() {
        let key = crate::signing::load_private_key(include_str!(
            "../tests/fixtures/test_slicer_key.pem"
        ))
        .unwrap();
        let pem = RsaPublicKey::from(&key)
            .to_public_key_pem(LineEnding::LF)
            .unwrap();
        let found = harvest_bootstrap(pem.as_bytes(), &[]).expect("wrap pem");
        assert!(found
            .server_wrap_pem
            .as_ref()
            .is_some_and(|s| s.contains("BEGIN PUBLIC KEY")));
    }

    #[test]
    fn harvest_bootstrap_from_helper_dump_if_present() {
        for (path, rand_path) in [
            ("/tmp/bambu-vmp-new.dump", "/tmp/bambu-vmp-new.rand"),
            ("/tmp/bambu-vmp.dump", ""),
            ("/tmp/vmp-d.bin", ""),
            ("/tmp/bambu-vmp2.dump", ""),
        ] {
            let Ok(bytes) = std::fs::read(path) else {
                continue;
            };
            let rand = if rand_path.is_empty() {
                Vec::new()
            } else {
                std::fs::read(rand_path).unwrap_or_default()
            };
            let urls = cert_url_blobs(&bytes).len();
            let found = harvest_bootstrap(&bytes, &rand);
            let secret_len = found
                .as_ref()
                .and_then(|c| c.client_auth_secret.as_ref())
                .map(|s| s.len())
                .unwrap_or(0);
            let wrap = found
                .as_ref()
                .and_then(|c| c.server_wrap_pem.as_ref())
                .is_some();
            if secret_len == 0 && !wrap {
                eprintln!("{path}: urls={urls} no bootstrap in dump (hook/rand needed)");
                return;
            }
            assert!(
                secret_len >= 40 || wrap,
                "{path}: bootstrap secret too short (len={secret_len})"
            );
            if secret_len > 0 {
                let s = found.unwrap().client_auth_secret.unwrap();
                assert!(s.starts_with(b"GLOF"));
                assert!(s.iter().all(|b| b.is_ascii_graphic()));
            }
            return;
        }
    }
}
