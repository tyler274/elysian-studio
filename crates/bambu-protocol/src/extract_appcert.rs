//! Unwrap the `get_app_cert` JSON `key` blob (ClusterM §10.2 / `fetch_slicer_credentials.py`).
//!
//! Cipher: AES-256-CTR with a non-standard S-box, fixed key `00..1f`, counter starting at 2.
//! The request `session_key` is **not** used. KATs match the OBN Python self-test.

use aes_gcm::aead::{Aead, KeyInit};
use aes_gcm::{Aes256Gcm, Nonce};
use base64::Engine;
use rand::RngCore;
use rsa::pkcs1::DecodeRsaPublicKey;
use rsa::pkcs8::{DecodePublicKey, EncodePrivateKey, LineEnding};
use rsa::traits::PublicKeyParts;
use rsa::{BigUint, Pkcs1v15Encrypt, RsaPrivateKey, RsaPublicKey};

use crate::credentials::{CredentialError, SlicerCredentials};
use crate::extract_elf::{find_bytes, json_b64_fields};
use crate::signing::{load_private_key, public_key_from_cert_pem};

const SBOX: [u8; 256] = [
    0xC5, 0x57, 0x4D, 0x6C, 0x3A, 0x95, 0x05, 0xE0, 0xA3, 0xBA, 0x36, 0x1F, 0xEA, 0x51, 0x53, 0x3B,
    0x0E, 0x07, 0x4E, 0x64, 0x50, 0x04, 0x40, 0xE8, 0x62, 0x6E, 0x9F, 0x2D, 0x70, 0x8B, 0x28, 0x49,
    0xD5, 0xF9, 0x65, 0x8D, 0x74, 0x68, 0x7C, 0x6F, 0x0A, 0x6A, 0xB3, 0xAF, 0x38, 0xFE, 0x7E, 0x8A,
    0x47, 0x7F, 0xB0, 0x16, 0x00, 0xD4, 0x0F, 0x13, 0xC9, 0x80, 0x4A, 0xAC, 0x8C, 0x4F, 0xA7, 0x98,
    0x83, 0x94, 0x5D, 0x48, 0xB4, 0xE9, 0x30, 0x19, 0x03, 0x99, 0x25, 0xBF, 0x8E, 0x41, 0xA0, 0xE4,
    0xC3, 0xCF, 0x2C, 0xAB, 0xD2, 0x32, 0x1A, 0x0C, 0x11, 0xB5, 0x56, 0x63, 0x15, 0xA6, 0x69, 0x0B,
    0x88, 0xBB, 0x4C, 0x10, 0xCB, 0x75, 0xFA, 0x81, 0xF8, 0xCD, 0xA1, 0xD6, 0x97, 0xB7, 0x26, 0xC6,
    0x9E, 0xF1, 0x5F, 0xE5, 0xA9, 0x87, 0xC7, 0xDC, 0x8F, 0x7A, 0x86, 0x20, 0x9A, 0xD1, 0x08, 0xC2,
    0x84, 0x09, 0x33, 0x1B, 0xDD, 0x1E, 0xFD, 0x01, 0x71, 0xDA, 0x77, 0x0D, 0xD7, 0xDE, 0x93, 0xCA,
    0xA5, 0xD0, 0xE6, 0x60, 0x89, 0x37, 0xC8, 0x21, 0x59, 0x79, 0x96, 0xAD, 0x24, 0x34, 0xB9, 0x44,
    0xFC, 0xC1, 0xAE, 0xF3, 0x82, 0x46, 0x43, 0x31, 0xE3, 0x2E, 0x4B, 0xFB, 0x92, 0x55, 0xED, 0x45,
    0x76, 0x6D, 0xAA, 0x3F, 0xF5, 0x5A, 0x91, 0x78, 0x22, 0x06, 0xFF, 0xD9, 0x35, 0x7D, 0x7B, 0xDB,
    0x54, 0x12, 0x9C, 0xD8, 0xD3, 0xEE, 0x17, 0x42, 0x52, 0x3E, 0xA4, 0xE7, 0xDF, 0x9D, 0xF2, 0xF4,
    0xEF, 0x73, 0xF6, 0x5E, 0xB1, 0x5B, 0x18, 0xE2, 0x9B, 0x58, 0xA8, 0x2A, 0xE1, 0x3D, 0x90, 0xB6,
    0x1C, 0xBD, 0x61, 0xEB, 0x23, 0xA2, 0x67, 0x39, 0xF0, 0xBC, 0xB2, 0xF7, 0x85, 0x27, 0x72, 0xCC,
    0x29, 0xB8, 0x1D, 0xBE, 0x66, 0xC4, 0x2F, 0xCE, 0x14, 0x3C, 0x6B, 0xEC, 0x5C, 0x2B, 0xC0, 0x02,
];

const RCON: [u8; 14] = [
    0x01, 0x02, 0x04, 0x08, 0x10, 0x20, 0x40, 0x80, 0x1B, 0x36, 0x6C, 0xD8, 0xAB, 0x4D,
];

const APPCERT_KEY: [u8; 32] = [
    0, 1, 2, 3, 4, 5, 6, 7, 8, 9, 10, 11, 12, 13, 14, 15, 16, 17, 18, 19, 20, 21, 22, 23, 24, 25,
    26, 27, 28, 29, 30, 31,
];

fn xtime(a: u8) -> u8 {
    let hi = a & 0x80;
    let s = a << 1;
    if hi != 0 {
        s ^ 0x1B
    } else {
        s
    }
}

fn gmul(mut a: u8, mut b: u8) -> u8 {
    let mut r = 0u8;
    while b != 0 {
        if b & 1 != 0 {
            r ^= a;
        }
        a = xtime(a);
        b >>= 1;
    }
    r
}

fn key_expand(key: &[u8; 32]) -> [[u8; 16]; 15] {
    let mut w = [[0u8; 4]; 60];
    for i in 0..8 {
        for j in 0..4 {
            w[i][j] = key[4 * i + j];
        }
    }
    for i in 8..60 {
        let mut t = w[i - 1];
        if i % 8 == 0 {
            let tmp = t[0];
            t[0] = SBOX[t[1] as usize] ^ RCON[i / 8 - 1];
            t[1] = SBOX[t[2] as usize];
            t[2] = SBOX[t[3] as usize];
            t[3] = SBOX[tmp as usize];
        } else if i % 8 == 4 {
            t = [
                SBOX[t[0] as usize],
                SBOX[t[1] as usize],
                SBOX[t[2] as usize],
                SBOX[t[3] as usize],
            ];
        }
        for j in 0..4 {
            w[i][j] = w[i - 8][j] ^ t[j];
        }
    }
    let mut rk = [[0u8; 16]; 15];
    for r in 0..15 {
        for c in 0..4 {
            for j in 0..4 {
                rk[r][4 * c + j] = w[4 * r + c][j];
            }
        }
    }
    rk
}

fn aes_encrypt_block(block: &[u8; 16], rk: &[[u8; 16]; 15]) -> [u8; 16] {
    let mut s = [0u8; 16];
    for i in 0..16 {
        s[i] = block[i] ^ rk[0][i];
    }
    for rnd in 1..15 {
        for x in &mut s {
            *x = SBOX[*x as usize];
        }
        let mut t = [0u8; 16];
        for r in 0..4 {
            for c in 0..4 {
                t[4 * c + r] = s[4 * ((c + r) % 4) + r];
            }
        }
        s = t;
        if rnd < 14 {
            for c in 0..4 {
                let a0 = s[4 * c];
                let a1 = s[4 * c + 1];
                let a2 = s[4 * c + 2];
                let a3 = s[4 * c + 3];
                s[4 * c] = gmul(a0, 2) ^ gmul(a1, 3) ^ a2 ^ a3;
                s[4 * c + 1] = a0 ^ gmul(a1, 2) ^ gmul(a2, 3) ^ a3;
                s[4 * c + 2] = a0 ^ a1 ^ gmul(a2, 2) ^ gmul(a3, 3);
                s[4 * c + 3] = gmul(a0, 3) ^ a1 ^ a2 ^ gmul(a3, 2);
            }
        }
        for i in 0..16 {
            s[i] ^= rk[rnd][i];
        }
    }
    s
}

fn ctr_xor(key: &[u8; 32], nonce: &[u8], data: &[u8]) -> Vec<u8> {
    let rk = key_expand(key);
    let mut out = vec![0u8; data.len()];
    let blocks = data.len().div_ceil(16);
    for i in 0..blocks {
        let mut ctr = [0u8; 16];
        ctr[..12].copy_from_slice(&nonce[..12]);
        let n = (2 + i as u32).to_be_bytes();
        ctr[12..].copy_from_slice(&n);
        let ks = aes_encrypt_block(&ctr, &rk);
        let base = i * 16;
        let take = (data.len() - base).min(16);
        for j in 0..take {
            out[base + j] = data[base + j] ^ ks[j];
        }
    }
    out
}

fn unwrap_skey_blob(blob: &[u8]) -> Option<String> {
    if blob.len() < 32 + 288 {
        return None;
    }
    let nonce = &blob[..12];
    let mut ct_len = u32::from_le_bytes(blob[28..32].try_into().ok()?) as usize;
    if 32 + ct_len > blob.len() {
        ct_len = blob.len() - 32;
    }
    let pt = ctr_xor(&APPCERT_KEY, nonce, &blob[32..32 + ct_len]);
    if pt.len() < 4 || pt[..4] != *b"\x59\x45\x4b\x53" {
        return None;
    }
    const HDR: usize = 32;
    const LIMB: usize = 128;
    if pt.len() < HDR + 5 * LIMB {
        return None;
    }
    let p = BigUint::from_bytes_be(&pt[HDR..HDR + LIMB]);
    let q = BigUint::from_bytes_be(&pt[HDR + LIMB..HDR + 2 * LIMB]);
    let e = BigUint::from(65537u32);
    let key = RsaPrivateKey::from_p_q(p, q, e).ok()?;
    key.to_pkcs8_pem(LineEnding::LF)
        .ok()
        .map(|pem| pem.to_string())
}

/// Scan a dump (or live heap chunk) for JSON `"key"` blobs and unwrap the app private key.
///
/// `cert_pem` is a preference only. Live harvest often keeps the GLOF *intermediate*
/// first; that modulus will not match the leaf private key, so a mismatch falls back
/// to any blob that unwraps to a valid RSA key (ClusterM `fetch_slicer_credentials.py`).
pub fn try_unwrap_appcert_key(data: &[u8], cert_pem: Option<&str>) -> Option<String> {
    let want = cert_pem.and_then(|c| public_key_from_cert_pem(c).ok());
    let mut fallback = None;
    for blob in json_key_blobs(data) {
        let Some(pem) = unwrap_skey_blob(&blob) else {
            continue;
        };
        let Ok(key) = load_private_key(&pem) else {
            continue;
        };
        if let Some(ref want) = want {
            if key.n() == want.n() {
                return Some(pem);
            }
            fallback.get_or_insert(pem);
            continue;
        }
        return Some(pem);
    }
    fallback
}

fn json_key_blobs(data: &[u8]) -> Vec<Vec<u8>> {
    let mut out = json_b64_fields(data, br#""key":""#);
    for extra in json_b64_fields(data, br#""key": ""#) {
        if !out.iter().any(|b| b == &extra) {
            out.push(extra);
        }
    }
    out
}

/// Unwrap the `get_app_cert` JSON object (cert chain + CRL + `key` blob) from a dump.
pub fn harvest_appcert(data: &[u8]) -> Option<SlicerCredentials> {
    let key = try_unwrap_appcert_key(data, None)?;
    let key_n = load_private_key(&key).ok()?.n().clone();
    let cert = json_cert_matching(data, &key_n).or_else(|| pem_leaf_matching(data, &key_n));
    let crl = json_string_fields(data, br#""crl":[""#)
        .into_iter()
        .chain(json_string_fields(data, br#""crl": [""#))
        .find(|s| s.contains("BEGIN X509 CRL") || s.contains("BEGIN CRL"));
    Some(SlicerCredentials {
        cert_pem: cert,
        key_pem: Some(key),
        crl_pem: crl,
        ..SlicerCredentials::default()
    })
}

fn json_cert_matching(data: &[u8], want: &BigUint) -> Option<String> {
    json_string_fields(data, br#""cert":""#)
        .into_iter()
        .chain(json_string_fields(data, br#""cert": ""#))
        .find_map(|s| chain_matching_leaf(&s, want))
}

fn chain_matching_leaf(pem: &str, want: &BigUint) -> Option<String> {
    if public_key_from_cert_pem(pem)
        .ok()
        .is_some_and(|p| p.n() == want)
    {
        return Some(pem.to_string());
    }
    None
}

fn pem_leaf_matching(data: &[u8], want: &BigUint) -> Option<String> {
    let text = String::from_utf8_lossy(data);
    let bytes = text.as_bytes();
    let mut i = 0;
    while i < bytes.len() {
        let rest = &text[i..];
        let Some(begin) = rest.find("-----BEGIN CERTIFICATE-----") else {
            break;
        };
        let start = i + begin;
        let after = &text[start..];
        let Some(end_rel) = after.find("-----END CERTIFICATE-----") else {
            break;
        };
        let end = start + end_rel + "-----END CERTIFICATE-----".len();
        if end <= text.len() {
            let block = text[start..end].trim().to_string() + "\n";
            if public_key_from_cert_pem(&block)
                .ok()
                .is_some_and(|p| p.n() == want)
            {
                return Some(block);
            }
        }
        i = end.max(start + 1);
    }
    None
}

fn json_string_fields(data: &[u8], prefix: &[u8]) -> Vec<String> {
    let mut out = Vec::new();
    let mut from = 0;
    while let Some(rel) = find_bytes(&data[from..], prefix) {
        let start = from + rel + prefix.len();
        match read_json_string(&data[start..]) {
            Some((s, n)) => {
                if s.len() >= 32 {
                    out.push(s);
                }
                from = start + n.max(1);
            }
            None => from = start.max(from + 1) + 1,
        }
    }
    out
}

fn read_json_string(data: &[u8]) -> Option<(String, usize)> {
    let mut out = Vec::new();
    let mut i = 0;
    while i < data.len() {
        match data[i] {
            b'\\' => {
                i += 1;
                let c = *data.get(i)?;
                match c {
                    b'n' => out.push(b'\n'),
                    b'r' => out.push(b'\r'),
                    b't' => out.push(b'\t'),
                    b'"' | b'\\' | b'/' => out.push(c),
                    _ => return None,
                }
                i += 1;
            }
            b'"' => {
                let s = String::from_utf8(out).ok()?;
                return Some((s, i + 1));
            }
            b => {
                out.push(b);
                i += 1;
            }
        }
        if out.len() > 64 * 1024 {
            return None;
        }
    }
    None
}

/// ClusterM `tools/fetch_slicer_credentials.py`: envelope GET, then the same SKEY unwrap.
pub fn fetch_appcert_from_bootstrap(
    secret: &[u8],
    wrap_pem: &[u8],
    api_host: &str,
) -> Result<SlicerCredentials, CredentialError> {
    let path = build_cert_request_path(secret, wrap_pem)?;
    let host = api_host
        .strip_prefix("https://")
        .unwrap_or(api_host)
        .trim_end_matches('/');
    let resp = crate::https::request(
        "GET",
        host,
        &path,
        &[("User-Agent", "bambu-studio-rs")],
        None,
    )
    .map_err(|err| CredentialError::Message(format!("app cert GET: {err}")))?;
    if resp.status != 200 {
        return Err(CredentialError::Message(format!(
            "app cert GET HTTP {}",
            resp.status
        )));
    }
    creds_from_appcert_json(&resp.body)
}

pub fn try_bootstrap_secret_files(dirs: &[std::path::PathBuf]) -> Option<(Vec<u8>, Vec<u8>)> {
    for dir in dirs {
        let secret = dir.join("client_auth_secret.txt");
        let wrap = dir.join("server_wrap_key.pem");
        if secret.is_file() && wrap.is_file() {
            let mut s = std::fs::read(&secret).ok()?;
            if s.ends_with(b"\n") {
                s.pop();
            }
            if s.ends_with(b"\r") {
                s.pop();
            }
            let w = std::fs::read(&wrap).ok()?;
            if !s.is_empty() && !w.is_empty() {
                return Some((s, w));
            }
        }
    }
    None
}

fn creds_from_appcert_json(body: &[u8]) -> Result<SlicerCredentials, CredentialError> {
    let v: serde_json::Value = serde_json::from_slice(body)
        .map_err(|err| CredentialError::Message(format!("app cert JSON: {err}")))?;
    let code_ok = v.get("code").map_or(true, |c| {
        c.as_i64() == Some(0) || c.as_u64() == Some(0) || c.as_str() == Some("0")
    }) || v.get("message").and_then(|m| m.as_str()) == Some("success");
    if !code_ok {
        return Err(CredentialError::Message("app cert API error".into()));
    }
    let cert = json_text_or_join(v.get("cert"))
        .ok_or_else(|| CredentialError::Message("app cert response missing cert".into()))?;
    let crl = json_text_or_join(v.get("crl"));
    let key_b64 = v
        .get("key")
        .and_then(|k| k.as_str())
        .ok_or_else(|| CredentialError::Message("app cert response missing key".into()))?;
    let blob = b64decode_any(key_b64)
        .ok_or_else(|| CredentialError::Message("app cert key blob was not base64".into()))?;
    let key = unwrap_skey_blob(&blob)
        .ok_or_else(|| CredentialError::Message("app cert key blob unwrap failed".into()))?;
    let want =
        public_key_from_cert_pem(&cert).map_err(|err| CredentialError::Message(err.to_string()))?;
    let got = load_private_key(&key).map_err(|err| CredentialError::Message(err.to_string()))?;
    if got.n() != want.n() {
        return Err(CredentialError::Message(
            "unwrapped key does not match leaf certificate".into(),
        ));
    }
    Ok(SlicerCredentials {
        cert_pem: Some(cert),
        key_pem: Some(key),
        crl_pem: crl,
        ..SlicerCredentials::default()
    })
}

fn json_text_or_join(v: Option<&serde_json::Value>) -> Option<String> {
    match v? {
        serde_json::Value::String(s) => Some(s.clone()),
        serde_json::Value::Array(items) => {
            let s = items
                .iter()
                .filter_map(|x| x.as_str())
                .collect::<Vec<_>>()
                .join("");
            if s.is_empty() {
                None
            } else {
                Some(s)
            }
        }
        _ => None,
    }
}

pub(crate) fn b64decode_any(s: &str) -> Option<Vec<u8>> {
    let t = s.trim().replace('-', "+").replace('_', "/");
    let pad = (4 - t.len() % 4) % 4;
    let t = t + &"=".repeat(pad);
    base64::engine::general_purpose::STANDARD.decode(t).ok()
}

fn build_cert_request_path(secret: &[u8], wrap_pem: &[u8]) -> Result<String, CredentialError> {
    let mut session_key = [0u8; 32];
    let mut iv = [0u8; 12];
    rand::rngs::OsRng.fill_bytes(&mut session_key);
    rand::rngs::OsRng.fill_bytes(&mut iv);
    let cipher = Aes256Gcm::new_from_slice(&session_key)
        .map_err(|err| CredentialError::Message(format!("AES-GCM: {err}")))?;
    let nonce = Nonce::from_slice(&iv);
    let ct_tag = cipher
        .encrypt(nonce, secret)
        .map_err(|err| CredentialError::Message(format!("AES-GCM: {err}")))?;
    let mut enc = Vec::with_capacity(12 + ct_tag.len());
    enc.extend_from_slice(&iv);
    enc.extend_from_slice(&ct_tag);
    let pub_key = RsaPublicKey::from_public_key_pem(std::str::from_utf8(wrap_pem).unwrap_or(""))
        .or_else(|_| RsaPublicKey::from_pkcs1_pem(std::str::from_utf8(wrap_pem).unwrap_or("")))
        .map_err(|err| CredentialError::Message(format!("server wrap key: {err}")))?;
    let wrapped = pub_key
        .encrypt(&mut rand::rngs::OsRng, Pkcs1v15Encrypt, &session_key)
        .map_err(|err| CredentialError::Message(format!("wrap session key: {err}")))?;
    let enc_secret = base64::engine::general_purpose::URL_SAFE.encode(&enc);
    let aes256 = base64::engine::general_purpose::URL_SAFE.encode(&wrapped);
    Ok(format!(
        "/v1/iot-service/api/user/applications/{enc_secret}/cert?aes256={aes256}&ver=1"
    ))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn sbox_is_bijection_and_not_aes() {
        let mut seen = [false; 256];
        for b in SBOX {
            assert!(!seen[b as usize]);
            seen[b as usize] = true;
        }
        assert!(seen.iter().all(|s| *s));
        assert_ne!(SBOX[0], 0x63);
    }

    #[test]
    fn keystream_kats_from_obn_python() {
        let nonce = hex("303c8f522a171b9a40dc8901");
        let rk = key_expand(&APPCERT_KEY);
        let mut ctr2 = [0u8; 16];
        ctr2[..12].copy_from_slice(&nonce);
        ctr2[12..].copy_from_slice(&2u32.to_be_bytes());
        assert_eq!(
            &aes_encrypt_block(&ctr2, &rk)[..],
            hex("c6bf1cc1b40dcc16443034d6dabe7b81").as_slice()
        );
        let mut ctr3 = ctr2;
        ctr3[12..].copy_from_slice(&3u32.to_be_bytes());
        assert_eq!(
            &aes_encrypt_block(&ctr3, &rk)[..],
            hex("a5dee6b960b56cf67aa24081f505a4b8").as_slice()
        );
        let mut ctr4 = ctr2;
        ctr4[12..].copy_from_slice(&4u32.to_be_bytes());
        assert_eq!(
            &aes_encrypt_block(&ctr4, &rk)[..],
            hex("a155d92625a1eb00d8901fef52a8f105").as_slice()
        );
        let ct32 = hex("9ffa5792b50dcc16443834d65abe7b8125dee6b9e0b56cf6faa240817505a4b8");
        let pt32 = ctr_xor(&APPCERT_KEY, &nonce, &ct32);
        assert_eq!(
            pt32,
            hex("59454b5301000000000800008000000080000000800000008000000080000000")
        );
        assert_eq!(&pt32[..4], b"\x59\x45\x4b\x53");
    }

    #[test]
    fn ctr_round_trip() {
        let nonce = hex("000102030405060708090a0b");
        let mut pt = vec![0u8; 704];
        pt[0..4].copy_from_slice(b"\x59\x45\x4b\x53");
        pt[4] = 1;
        for i in 32..704 {
            pt[i] = ((i * 7 + 3) & 0xff) as u8;
        }
        let ct = ctr_xor(&APPCERT_KEY, &nonce, &pt);
        assert_ne!(ct, pt);
        assert_eq!(ctr_xor(&APPCERT_KEY, &nonce, &ct), pt);
        let wrong = ctr_xor(&APPCERT_KEY, &hex("303c8f522a171b9a40dc8901"), &ct);
        assert_ne!(&wrong[..4], b"\x59\x45\x4b\x53");
    }

    #[test]
    fn unwrap_existing_helper_dump_if_present() {
        for path in [
            "/tmp/vmp-d.bin",
            "/tmp/bambu-vmp.dump",
            "/tmp/bambu-vmp2.dump",
        ] {
            let Ok(bytes) = std::fs::read(path) else {
                continue;
            };
            let blobs = crate::extract_elf::json_b64_fields(&bytes, br#""key":""#);
            if blobs.is_empty() {
                continue;
            }
            let mut magics = Vec::new();
            for blob in &blobs {
                if blob.len() < 36 {
                    magics.push(format!("short:{}", blob.len()));
                    continue;
                }
                let ct_len = u32::from_le_bytes(blob[28..32].try_into().unwrap()) as usize;
                let end = (32 + ct_len).min(blob.len());
                let pt = ctr_xor(&APPCERT_KEY, &blob[..12], &blob[32..end]);
                magics.push(format!(
                    "len={} ct_len={ct_len} magic={:02x}{:02x}{:02x}{:02x}",
                    blob.len(),
                    pt.first().copied().unwrap_or(0),
                    pt.get(1).copied().unwrap_or(0),
                    pt.get(2).copied().unwrap_or(0),
                    pt.get(3).copied().unwrap_or(0)
                ));
            }
            let pem = try_unwrap_appcert_key(&bytes, None);
            assert!(
                pem.is_some(),
                "{path}: unwrap failed ({})",
                magics.join("; ")
            );
            let creds = harvest_appcert(&bytes).expect("harvest after unwrap");
            let key = load_private_key(creds.key_pem.as_ref().unwrap()).unwrap();
            let cert = creds
                .cert_pem
                .as_ref()
                .expect("JSON/PEM cert matching unwrapped key");
            let pubk = public_key_from_cert_pem(cert).unwrap();
            assert_eq!(key.n(), pubk.n());
            assert!(
                creds.crl_pem.as_ref().is_some_and(|s| s.contains("BEGIN")),
                "JSON crl missing"
            );
            return;
        }
    }

    #[test]
    fn cert_request_path_is_base64url() {
        use rsa::pkcs8::EncodePublicKey;
        let key = load_private_key(include_str!("../tests/fixtures/test_slicer_key.pem")).unwrap();
        let pub_pem = RsaPublicKey::from(&key)
            .to_public_key_pem(LineEnding::LF)
            .unwrap();
        let path = build_cert_request_path(
            b"GLOF3813734089-836763c70000deadbeefdeadbeef",
            pub_pem.as_bytes(),
        )
        .unwrap();
        assert!(path.starts_with("/v1/iot-service/api/user/applications/"));
        assert!(path.contains("/cert?aes256="));
        assert!(path.ends_with("&ver=1"));
        let enc = path
            .split("/applications/")
            .nth(1)
            .unwrap()
            .split('/')
            .next()
            .unwrap();
        assert!(
            !enc.contains('/'),
            "standard base64 would 404 on the gateway"
        );
        assert!(!enc.contains('+'));
    }

    fn hex(s: &str) -> Vec<u8> {
        (0..s.len())
            .step_by(2)
            .map(|i| u8::from_str_radix(&s[i..i + 2], 16).unwrap())
            .collect()
    }
}
