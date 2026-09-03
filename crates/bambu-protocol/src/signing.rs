//! MQTT print-command signing matching ClusterM/open-bamboo-networking `signing.cpp`.
//!
//! Envelope:
//! `{"header":{"cert_id","payload_len","sign_alg":"RSA_SHA256","sign_string","sign_ver":"v1.0"},"print":{...sorted keys...}}`

use base64::Engine;
use rsa::pkcs1::DecodeRsaPrivateKey;
use rsa::pkcs8::DecodePrivateKey;
use rsa::traits::PublicKeyParts;
use rsa::{BigUint, Pkcs1v15Encrypt, RsaPrivateKey, RsaPublicKey};
use serde_json::{Map, Value};
use sha2::{Digest, Sha256};
use thiserror::Error;
use x509_parser::prelude::*;
use x509_parser::public_key::PublicKey;

use crate::credentials::SlicerCredentials;

#[derive(Debug, Error)]
pub enum SigningError {
    #[error("no slicer_key.pem loaded")]
    MissingKey,
    #[error("rsa: {0}")]
    Rsa(String),
    #[error("json: {0}")]
    Json(#[from] serde_json::Error),
    #[error("{0}")]
    Message(String),
}

pub fn load_private_key(pem: &str) -> Result<RsaPrivateKey, SigningError> {
    RsaPrivateKey::from_pkcs8_pem(pem)
        .or_else(|_| RsaPrivateKey::from_pkcs1_pem(pem))
        .map_err(|err| SigningError::Rsa(err.to_string()))
}

/// MQTT `cert_id` = lowercase hex serial + issuer RFC2253, no separator.
pub fn slicer_cert_id(cert_pem: &str) -> Result<String, SigningError> {
    let (_, pem) = parse_x509_pem(cert_pem.as_bytes())
        .map_err(|err| SigningError::Message(format!("cert pem: {err}")))?;
    let (_, cert) = parse_x509_certificate(&pem.contents)
        .map_err(|err| SigningError::Message(format!("x509: {err}")))?;
    let serial = cert.raw_serial().iter().fold(String::new(), |mut acc, b| {
        acc.push_str(&format!("{b:02x}"));
        acc
    });
    let issuer = rfc2253_name(cert.issuer());
    Ok(serial + &issuer)
}

fn rfc2253_name(name: &x509_parser::x509::X509Name<'_>) -> String {
    // OpenSSL XN_FLAG_RFC2253: most-significant RDN last (C=..,O=..,CN=..).
    name.iter_rdn()
        .map(|rdn| {
            rdn.iter()
                .map(|attr| {
                    let oid = attr.attr_type();
                    let tag = match oid.to_id_string().as_str() {
                        "2.5.4.3" => "CN",
                        "2.5.4.6" => "C",
                        "2.5.4.10" => "O",
                        "2.5.4.11" => "OU",
                        "2.5.4.7" => "L",
                        "2.5.4.8" => "ST",
                        _ => "OID",
                    };
                    let val = attr
                        .as_str()
                        .map(ToString::to_string)
                        .unwrap_or_else(|_| String::new());
                    format!("{tag}={val}")
                })
                .collect::<Vec<_>>()
                .join("+")
        })
        .collect::<Vec<_>>()
        .join(",")
}

pub fn rsa_sha256_sign_b64(key: &RsaPrivateKey, data: &[u8]) -> Result<String, SigningError> {
    let hash = Sha256::digest(data);
    // DigestInfo prefix for SHA-256 (RFC 8017).
    let mut digest_info = vec![
        0x30, 0x31, 0x30, 0x0d, 0x06, 0x09, 0x60, 0x86, 0x48, 0x01, 0x65, 0x03, 0x04, 0x02, 0x01,
        0x05, 0x00, 0x04, 0x20,
    ];
    digest_info.extend_from_slice(&hash);
    let sig = key
        .sign(rsa::Pkcs1v15Sign::new_unprefixed(), &digest_info)
        .map_err(|err| SigningError::Rsa(err.to_string()))?;
    Ok(base64::engine::general_purpose::STANDARD.encode(sig))
}

pub fn public_key_from_cert_pem(pem: &str) -> Result<RsaPublicKey, SigningError> {
    let mut rest = pem.as_bytes();
    while !rest.is_empty() {
        let (remaining, pem) = parse_x509_pem(rest)
            .map_err(|err| SigningError::Message(format!("cert pem: {err}")))?;
        rest = remaining;
        if !pem.label.contains("CERTIFICATE") {
            continue;
        }
        let (_, cert) = parse_x509_certificate(&pem.contents)
            .map_err(|err| SigningError::Message(format!("x509: {err}")))?;
        match cert.public_key().parsed() {
            Ok(PublicKey::RSA(rsa)) => {
                let n = BigUint::from_bytes_be(rsa.modulus);
                let e = BigUint::from_bytes_be(rsa.exponent);
                return RsaPublicKey::new(n, e).map_err(|err| SigningError::Rsa(err.to_string()));
            }
            Ok(_) => continue,
            Err(err) => return Err(SigningError::Message(format!("public key: {err}"))),
        }
    }
    Err(SigningError::Message(
        "no RSA certificate in device cert PEM".into(),
    ))
}

/// ClusterM `EncryptField`: PKCS#1 v1.5 blocks of `key_len - 11` bytes, concatenated, Base64.
pub fn encrypt_field(pub_key: &RsaPublicKey, plaintext: &[u8]) -> Result<String, SigningError> {
    let max_pt = pub_key.size().saturating_sub(11);
    if max_pt == 0 {
        return Err(SigningError::Rsa("RSA key too small".into()));
    }
    let mut rng = rand::rngs::OsRng;
    let mut out = Vec::new();
    for chunk in plaintext.chunks(max_pt) {
        let block = pub_key
            .encrypt(&mut rng, Pkcs1v15Encrypt, chunk)
            .map_err(|err| SigningError::Rsa(err.to_string()))?;
        out.extend_from_slice(&block);
    }
    Ok(base64::engine::general_purpose::STANDARD.encode(out))
}

/// Sign a `{"print":...}` payload. Other JSON is returned unchanged.
pub fn maybe_sign(payload_json: &str, creds: &SlicerCredentials) -> Result<String, SigningError> {
    maybe_sign_ex(payload_json, creds, None, false)
}

/// `secured` is `fun` bit 29 set (Developer Mode **off**). Then `url`/`param` become `*_enc`.
pub fn maybe_sign_ex(
    payload_json: &str,
    creds: &SlicerCredentials,
    device_cert_pem: Option<&str>,
    secured: bool,
) -> Result<String, SigningError> {
    if !is_print_payload(payload_json) {
        return Ok(payload_json.to_string());
    }
    let Some(key_pem) = creds.key_pem.as_deref() else {
        return Ok(payload_json.to_string());
    };
    let key = load_private_key(key_pem)?;
    let mut root: Value = serde_json::from_str(payload_json)?;
    let Some(print) = root.get_mut("print") else {
        return Ok(payload_json.to_string());
    };
    if let Some(obj) = print.as_object_mut() {
        encrypt_print_fields(obj, device_cert_pem, secured)?;
    }
    let print_dump = serde_json::to_string(print)?;
    let to_sign = format!("{{\"print\":{print_dump}}}");
    let cert_id = creds
        .cert_pem
        .as_deref()
        .map(slicer_cert_id)
        .transpose()?
        .unwrap_or_default();
    let sig = rsa_sha256_sign_b64(&key, to_sign.as_bytes())?;
    Ok(format!(
        "{{\"header\":{{\"cert_id\":\"{}\",\"payload_len\":{},\"sign_alg\":\"RSA_SHA256\",\"sign_string\":\"{}\",\"sign_ver\":\"v1.0\"}},\"print\":{print_dump}}}",
        json_escape(&cert_id),
        to_sign.len(),
        json_escape(&sig)
    ))
}

fn encrypt_print_fields(
    obj: &mut Map<String, Value>,
    device_cert_pem: Option<&str>,
    secured: bool,
) -> Result<(), SigningError> {
    if !secured {
        return Ok(());
    }
    let Some(pem) = device_cert_pem else {
        return Ok(());
    };
    let pub_key = public_key_from_cert_pem(pem)?;
    let command = obj.get("command").and_then(Value::as_str).unwrap_or("");
    match command {
        "gcode_line" => {
            if let Some(Value::String(param)) = obj.remove("param") {
                let enc = encrypt_field(&pub_key, param.as_bytes())?;
                obj.insert("param_enc".into(), Value::String(enc));
            }
        }
        "project_file" => {
            if let Some(Value::String(url)) = obj.remove("url") {
                let enc = encrypt_field(&pub_key, url.as_bytes())?;
                obj.insert("url_enc".into(), Value::String(enc));
            }
        }
        _ => {}
    }
    Ok(())
}

fn is_print_payload(payload: &str) -> bool {
    let trimmed = payload.trim_start();
    let rest = trimmed.strip_prefix('{').unwrap_or(trimmed).trim_start();
    rest.starts_with("\"print\"")
}

fn json_escape(s: &str) -> String {
    s.replace('\\', "\\\\").replace('"', "\\\"")
}

#[cfg(test)]
mod tests {
    use super::*;

    fn test_creds() -> SlicerCredentials {
        SlicerCredentials {
            cert_pem: Some(include_str!("../tests/fixtures/test_slicer_cert.pem").into()),
            key_pem: Some(include_str!("../tests/fixtures/test_slicer_key.pem").into()),
            crl_pem: None,
        }
    }

    #[test]
    fn cert_id_matches_openssl_serial_and_issuer() {
        let pem = include_str!("../tests/fixtures/test_slicer_cert.pem");
        let id = slicer_cert_id(pem).unwrap();
        assert!(
            id.starts_with("13f91456aea791109e924ace8665c02bdad93045"),
            "{id}"
        );
        assert!(id.contains("CN=bambu-studio-rs-test"), "{id}");
        assert!(id.contains("O=Test"), "{id}");
        assert!(id.contains("C=US"), "{id}");
    }

    #[test]
    fn signs_print_payload() {
        let signed = maybe_sign(
            r#"{"print":{"command":"gcode_line","param":"G28","sequence_id":"1"}}"#,
            &test_creds(),
        )
        .unwrap();
        assert!(signed.contains("\"sign_alg\":\"RSA_SHA256\""));
        assert!(signed.contains("\"sign_ver\":\"v1.0\""));
        assert!(signed.contains("\"command\":\"gcode_line\""));
        let v: Value = serde_json::from_str(&signed).unwrap();
        assert!(v["header"]["sign_string"].as_str().unwrap().len() > 80);
        assert_eq!(
            v["header"]["payload_len"].as_u64().unwrap() as usize,
            format!(
                "{{\"print\":{}}}",
                serde_json::to_string(&v["print"]).unwrap()
            )
            .len()
        );
    }

    #[test]
    fn passthrough_without_key() {
        let json = r#"{"print":{"command":"gcode_line","param":"G28"}}"#;
        let out = maybe_sign(json, &SlicerCredentials::default()).unwrap();
        assert_eq!(out, json);
    }

    #[test]
    fn ignores_non_print() {
        let json = r#"{"info":{"command":"get_version"}}"#;
        let out = maybe_sign(json, &test_creds()).unwrap();
        assert_eq!(out, json);
    }

    #[test]
    fn encrypt_field_roundtrip_with_test_key() {
        let key = load_private_key(test_creds().key_pem.as_deref().unwrap()).unwrap();
        let pub_key = RsaPublicKey::from(&key);
        let plain = b"ftp://cube.gcode.3mf";
        let b64 = encrypt_field(&pub_key, plain).unwrap();
        let raw = base64::engine::general_purpose::STANDARD
            .decode(b64)
            .unwrap();
        assert_eq!(raw.len(), pub_key.size());
        let dec = key.decrypt(Pkcs1v15Encrypt, &raw).unwrap();
        assert_eq!(dec, plain);
    }

    #[test]
    fn secured_gcode_line_drops_cleartext_param() {
        let cert = include_str!("../tests/fixtures/test_slicer_cert.pem");
        let signed = maybe_sign_ex(
            r#"{"print":{"command":"gcode_line","param":"G28","sequence_id":"1"}}"#,
            &test_creds(),
            Some(cert),
            true,
        )
        .unwrap();
        let v: Value = serde_json::from_str(&signed).unwrap();
        assert!(v["print"].get("param").is_none());
        assert!(v["print"]["param_enc"].as_str().unwrap().len() > 80);
    }

    #[test]
    fn secured_project_file_drops_cleartext_url() {
        let cert = include_str!("../tests/fixtures/test_slicer_cert.pem");
        let signed = maybe_sign_ex(
            r#"{"print":{"command":"project_file","url":"ftp://cube.gcode.3mf","sequence_id":"1"}}"#,
            &test_creds(),
            Some(cert),
            true,
        )
        .unwrap();
        let v: Value = serde_json::from_str(&signed).unwrap();
        assert!(v["print"].get("url").is_none());
        assert!(v["print"]["url_enc"].as_str().unwrap().len() > 80);
    }
}
