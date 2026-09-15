//! Public Bambu cloud HTTPS (OpenBambuAPI / Home Assistant Bambu Lab).
//!
//! Fixture-tested JSON only in `cargo test --offline`. Live calls stay in CLI/UI.

use std::path::PathBuf;
use std::sync::OnceLock;

use serde_json::{json, Value};
use thiserror::Error;

use crate::credentials::{default_config_dir, load_from_dir, SlicerCredentials};
use crate::https::{self, HttpsError};
use crate::signing::http_security_headers;

pub const API_HOST_US: &str = "api.bambulab.com";
pub const API_HOST_CN: &str = "api.bambulab.cn";

#[derive(Debug, Error)]
pub enum CloudApiError {
    #[error("cloud: {0}")]
    Message(String),
    #[error(transparent)]
    Https(#[from] HttpsError),
    #[error(transparent)]
    Json(#[from] serde_json::Error),
    #[error(transparent)]
    Io(#[from] std::io::Error),
}

#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct CloudDevice {
    pub dev_id: String,
    pub name: String,
    pub online: bool,
    pub dev_name: String,
    pub access_code: String,
    pub dev_ver: String,
}

impl CloudDevice {
    pub fn label(&self) -> String {
        let name = if self.name.is_empty() {
            self.dev_name.as_str()
        } else {
            self.name.as_str()
        };
        if name.is_empty() {
            self.dev_id.clone()
        } else {
            format!("{name} ({})", self.dev_id)
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct UploadTicket {
    pub put_url: String,
    pub public_url: String,
    pub extra_headers: Vec<(String, String)>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum CameraProto {
    #[default]
    Tutk,
    Agora,
}

#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct CameraCreds {
    pub proto: CameraProto,
    pub uid: String,
    pub authkey: String,
    pub passwd: String,
    pub region: String,
    pub channel: String,
    pub app_id: String,
    pub token: String,
    pub stream_key: String,
    pub stream_salt: String,
    pub user: String,
    pub device: String,
}

impl CameraCreds {
    pub fn bambu_url(&self) -> String {
        match self.proto {
            CameraProto::Agora if !self.channel.is_empty() => {
                let mut url = crate::camera::agora_url(
                    &self.channel,
                    &self.region,
                    &self.token,
                    &self.authkey,
                    &self.app_id,
                );
                if !self.user.is_empty() {
                    url.push_str("&user=");
                    url.push_str(&crate::oauth::percent_encode_query(&self.user));
                }
                if !self.stream_key.is_empty() {
                    url.push_str("&streamKey=");
                    url.push_str(&crate::oauth::percent_encode_query(&self.stream_key));
                }
                if !self.stream_salt.is_empty() {
                    url.push_str("&streamSalt=");
                    url.push_str(&crate::oauth::percent_encode_query(&self.stream_salt));
                }
                if !self.device.is_empty() {
                    url.push_str("&device=");
                    url.push_str(&crate::oauth::percent_encode_query(&self.device));
                }
                url
            }
            _ => crate::camera::tutk_url(&self.uid, &self.authkey, &self.passwd, &self.region),
        }
    }

    pub fn bambu_url_for_device(&self, serial: &str) -> String {
        crate::tutk::append_device_query(self.bambu_url(), serial)
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum LoginResult {
    Tokens {
        access_token: String,
        refresh_token: String,
        user_id: String,
    },
    NeedsCode {
        login_type: String,
    },
}

pub fn api_host(region: &str) -> &'static str {
    match region.trim().to_ascii_lowercase().as_str() {
        "cn" | "china" => API_HOST_CN,
        _ => API_HOST_US,
    }
}

pub fn bind_path() -> &'static str {
    "/v1/iot-service/api/user/bind"
}

pub fn upload_path() -> &'static str {
    "/v1/iot-service/api/file/upload"
}

pub fn ttcode_path() -> &'static str {
    "/v1/iot-service/api/user/ttcode"
}

pub fn login_path() -> &'static str {
    "/v1/user-service/user/login"
}

pub fn refresh_path() -> &'static str {
    "/v1/user-service/user/refreshtoken"
}

pub fn profile_path() -> &'static str {
    "/v1/user-service/my/profile"
}

pub fn ticket_path(ticket: &str) -> String {
    format!(
        "/v1/user-service/user/ticket/{}",
        crate::oauth::percent_encode_query(ticket)
    )
}

pub fn ticket_body(ticket: &str) -> Value {
    json!({ "ticket": ticket })
}

#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct CloudProfile {
    pub user_id: String,
    pub name: String,
    pub account: String,
}

pub fn login_body(account: &str, password: &str, code: Option<&str>) -> Value {
    let mut body = serde_json::Map::new();
    body.insert("account".into(), json!(account));
    if let Some(code) = code.filter(|c| !c.is_empty()) {
        body.insert("code".into(), json!(code));
    } else {
        body.insert("password".into(), json!(password));
    }
    Value::Object(body)
}

pub fn refresh_body(refresh_token: &str) -> Value {
    json!({ "refreshToken": refresh_token })
}

/// Numeric uid (MQTT topics use `u_{uid}`).
pub fn http_user_id(user_id: &str) -> String {
    let id = user_id.trim();
    id.strip_prefix("u_").unwrap_or(id).to_string()
}

/// iot-service `user-id` header: bambulab-cloud / Handy send `u_{uid}`.
pub fn iot_user_id(user_id: &str) -> String {
    let id = user_id.trim();
    if id.is_empty() {
        return String::new();
    }
    if id.starts_with("u_") {
        return id.to_string();
    }
    let numeric = http_user_id(id);
    if !numeric.is_empty() && numeric.bytes().all(|b| b.is_ascii_digit()) {
        return format!("u_{numeric}");
    }
    id.to_string()
}

pub fn upload_ticket_body(filename: &str, size: usize, md5_hex: &str) -> Value {
    json!({
        "filename": filename,
        "size": size,
        "hash": md5_hex,
    })
}

pub fn parse_bind_devices(v: &Value) -> Vec<CloudDevice> {
    let mut out = Vec::new();
    let candidates = [
        v.get("devices"),
        v.get("bind_list"),
        v.pointer("/data/devices"),
        v.pointer("/data/bind_list"),
        v.pointer("/data/list"),
    ];
    for list in candidates.into_iter().flatten() {
        let Some(arr) = list.as_array() else {
            continue;
        };
        for item in arr {
            if let Some(dev) = parse_device(item) {
                if !out.iter().any(|d: &CloudDevice| d.dev_id == dev.dev_id) {
                    out.push(dev);
                }
            }
        }
    }
    out
}

/// If `requested` is missing from `/bind`, use the first online bound printer.
/// Stale `cloud_serial` (e.g. an old P1) otherwise 403s ttcode against the H2 that is actually bound.
pub fn camera_serial_from_bind(requested: &str, devices: &[CloudDevice]) -> String {
    let requested = requested.trim();
    if devices.iter().any(|d| d.dev_id == requested) {
        return requested.to_string();
    }
    devices
        .iter()
        .find(|d| d.online && !d.dev_id.is_empty())
        .or_else(|| devices.iter().find(|d| !d.dev_id.is_empty()))
        .map(|d| d.dev_id.clone())
        .unwrap_or_else(|| requested.to_string())
}

fn parse_device(v: &Value) -> Option<CloudDevice> {
    let dev_id = string_field(v, &["dev_id", "devId", "device_id"])?;
    if dev_id.is_empty() {
        return None;
    }
    let name = string_field(v, &["name", "nickname", "dev_name"]).unwrap_or_default();
    let dev_name =
        string_field(v, &["dev_name", "dev_product_name", "dev_model_name"]).unwrap_or_default();
    let online = v
        .get("online")
        .and_then(Value::as_bool)
        .or_else(|| {
            v.get("online")
                .and_then(Value::as_str)
                .map(|s| s == "1" || s.eq_ignore_ascii_case("true"))
        })
        .unwrap_or(false);
    let access_code = string_field(v, &["dev_access_code", "access_code", "user_access_code"])
        .unwrap_or_default();
    let dev_ver =
        string_field(v, &["dev_ver", "fw_ver", "ota_version", "sw_ver"]).unwrap_or_default();
    Some(CloudDevice {
        dev_id,
        name,
        online,
        dev_name,
        access_code,
        dev_ver,
    })
}

pub fn parse_camera_creds(v: &Value) -> Option<CameraCreds> {
    let data = v.get("data").unwrap_or(v);
    let uid = string_field(data, &["ttcode", "uid", "tutk_id"]).unwrap_or_default();
    let authkey = string_field(data, &["authkey", "auth_key"]).unwrap_or_default();
    let passwd = string_field(data, &["passwd", "password"]).unwrap_or_default();
    let region = string_field(data, &["region"]).unwrap_or_default();
    let channel = string_field(data, &["channel_name", "channel"]).unwrap_or_default();
    let app_id = string_field(data, &["app_id", "appId", "license"]).unwrap_or_default();
    let token = string_field(data, &["token"]).unwrap_or_default();
    let stream_key = string_field(data, &["stream_key", "streamKey"]).unwrap_or_default();
    let stream_salt = string_field(data, &["stream_salt", "streamSalt"]).unwrap_or_default();
    let user = string_field(data, &["user", "local_uid"]).unwrap_or_default();
    let token = if token.is_empty() {
        stream_key.clone()
    } else {
        token
    };
    let kind = string_field(data, &["type", "proto"]).unwrap_or_default();
    let proto = if kind.eq_ignore_ascii_case("agora") || !channel.is_empty() {
        CameraProto::Agora
    } else {
        CameraProto::Tutk
    };
    if uid.is_empty() && channel.is_empty() {
        return None;
    }
    Some(CameraCreds {
        proto,
        uid,
        authkey,
        passwd,
        region,
        channel,
        app_id,
        token,
        stream_key,
        stream_salt,
        user,
        device: String::new(),
    })
}

pub fn parse_upload_ticket(v: &Value) -> Result<UploadTicket, CloudApiError> {
    let data = v.get("data").unwrap_or(v);
    let put_url = first_url(
        data,
        &["url", "upload_url", "put_url", "signed_url", "uploadUrl"],
    )
    .or_else(|| {
        data.pointer("/url/put")
            .and_then(Value::as_str)
            .map(str::to_string)
    })
    .ok_or_else(|| CloudApiError::Message("upload ticket missing PUT url".into()))?;
    let public_url = first_url(
        data,
        &["public_url", "file_url", "get_url", "download_url", "url"],
    )
    .or_else(|| {
        data.pointer("/url/get")
            .and_then(Value::as_str)
            .map(str::to_string)
    })
    .unwrap_or_else(|| put_url.clone());
    let mut extra_headers = Vec::new();
    if let Some(Value::Object(map)) = data.get("headers").or_else(|| data.get("header")) {
        for (k, val) in map {
            if let Some(s) = val.as_str() {
                extra_headers.push((k.clone(), s.to_string()));
            }
        }
    }
    Ok(UploadTicket {
        put_url,
        public_url,
        extra_headers,
    })
}

fn first_url(v: &Value, keys: &[&str]) -> Option<String> {
    for key in keys {
        match v.get(*key) {
            Some(Value::String(s)) if s.starts_with("https://") => return Some(s.clone()),
            _ => {}
        }
    }
    None
}

pub fn parse_login(v: &Value) -> Result<LoginResult, CloudApiError> {
    let data = v.get("data").unwrap_or(v);
    let login_type = string_field(data, &["loginType", "login_type"]).unwrap_or_default();
    let access = string_field(data, &["accessToken", "access_token"]).unwrap_or_default();
    if access.is_empty()
        && (login_type.eq_ignore_ascii_case("verifyCode")
            || login_type.eq_ignore_ascii_case("verifycode")
            || login_type.to_ascii_lowercase().contains("code"))
    {
        return Ok(LoginResult::NeedsCode { login_type });
    }
    if access.is_empty() {
        return Err(CloudApiError::Message(
            "login did not return an access token (email code may be required)".into(),
        ));
    }
    let refresh = string_field(data, &["refreshToken", "refresh_token"]).unwrap_or_default();
    let user_id = string_field(data, &["userId", "user_id", "uid"]).unwrap_or_default();
    Ok(LoginResult::Tokens {
        access_token: access,
        refresh_token: refresh,
        user_id,
    })
}

pub fn parse_profile(v: &Value) -> CloudProfile {
    let data = v.get("data").unwrap_or(v);
    let user_id = string_field(data, &["uidStr", "uid", "userId", "user_id"]).unwrap_or_default();
    let name = string_field(data, &["name", "nickname", "nickName"]).unwrap_or_default();
    let account = string_field(data, &["account", "email"]).unwrap_or_default();
    CloudProfile {
        user_id,
        name,
        account,
    }
}

pub fn jwt_user_id(token: &str) -> Option<String> {
    let v = jwt_payload(token)?;
    let id = string_field(&v, &["uid", "userId", "user_id"])
        .or_else(|| string_field(&v, &["username", "sub"]))?;
    if id.contains('@') {
        return string_field(&v, &["uid", "userId", "user_id"]);
    }
    Some(http_user_id(&id))
}

/// `user-id` for iot-service (`ttcode`): JWT `username`, as bambulab-cloud sends.
pub fn jwt_iot_user_id(token: &str) -> Option<String> {
    let v = jwt_payload(token)?;
    string_field(&v, &["username"]).or_else(|| string_field(&v, &["uid", "userId", "user_id"]))
}

fn jwt_payload(token: &str) -> Option<Value> {
    let payload = token.split('.').nth(1)?;
    let mut b64 = payload.replace('-', "+").replace('_', "/");
    while b64.len() % 4 != 0 {
        b64.push('=');
    }
    let bytes = base64::Engine::decode(&base64::engine::general_purpose::STANDARD, b64).ok()?;
    serde_json::from_slice(&bytes).ok()
}

fn enrich_user_id(result: LoginResult, profile: Option<CloudProfile>) -> LoginResult {
    let LoginResult::Tokens {
        access_token,
        refresh_token,
        mut user_id,
    } = result
    else {
        return result;
    };
    if user_id.is_empty() {
        if let Some(p) = profile {
            user_id = p.user_id;
        }
    }
    if user_id.is_empty() {
        if let Some(id) = jwt_user_id(&access_token) {
            user_id = id;
        }
    }
    LoginResult::Tokens {
        access_token,
        refresh_token,
        user_id,
    }
}

pub fn parse_refresh(v: &Value) -> Result<(String, String), CloudApiError> {
    match parse_login(v)? {
        LoginResult::Tokens {
            access_token,
            refresh_token,
            ..
        } => Ok((access_token, refresh_token)),
        LoginResult::NeedsCode { login_type } => Err(CloudApiError::Message(format!(
            "refresh returned loginType {login_type}"
        ))),
    }
}

fn string_field(v: &Value, keys: &[&str]) -> Option<String> {
    for key in keys {
        if let Some(s) = v.get(*key).and_then(Value::as_str) {
            let s = s.trim();
            if !s.is_empty() {
                return Some(s.to_string());
            }
        }
        if let Some(n) = v.get(*key).and_then(Value::as_i64) {
            return Some(n.to_string());
        }
    }
    None
}

/// RFC 1321 MD5 hex (cloud upload `hash` field).
pub fn md5_hex(data: &[u8]) -> String {
    let digest = md5(data);
    digest.iter().map(|b| format!("{b:02x}")).collect()
}

fn md5(data: &[u8]) -> [u8; 16] {
    let mut s = [0x6745_2301u32, 0xEFCD_AB89, 0x98BA_DCFE, 0x1032_5476];
    let mut buf = data.to_vec();
    let bit_len = (data.len() as u64) * 8;
    buf.push(0x80);
    while buf.len() % 64 != 56 {
        buf.push(0);
    }
    buf.extend_from_slice(&bit_len.to_le_bytes());
    for chunk in buf.as_chunks::<64>().0 {
        let mut w = [0u32; 16];
        for (i, word) in w.iter_mut().enumerate() {
            let o = i * 4;
            let mut bytes = [0u8; 4];
            bytes.copy_from_slice(&chunk[o..o + 4]);
            *word = u32::from_le_bytes(bytes);
        }
        let (mut a, mut b, mut c, mut d) = (s[0], s[1], s[2], s[3]);
        for i in 0..64 {
            let (f, g) = if i < 16 {
                ((b & c) | ((!b) & d), i)
            } else if i < 32 {
                ((d & b) | ((!d) & c), (5 * i + 1) % 16)
            } else if i < 48 {
                (b ^ c ^ d, (3 * i + 5) % 16)
            } else {
                (c ^ (b | (!d)), (7 * i) % 16)
            };
            let k = MD5_K[i];
            let sum = a
                .wrapping_add(f)
                .wrapping_add(k)
                .wrapping_add(w[g])
                .rotate_left(MD5_S[i]);
            let new_b = b.wrapping_add(sum);
            a = d;
            d = c;
            c = b;
            b = new_b;
        }
        s[0] = s[0].wrapping_add(a);
        s[1] = s[1].wrapping_add(b);
        s[2] = s[2].wrapping_add(c);
        s[3] = s[3].wrapping_add(d);
    }
    let mut out = [0u8; 16];
    for (i, word) in s.iter().enumerate() {
        out[i * 4..(i + 1) * 4].copy_from_slice(&word.to_le_bytes());
    }
    out
}

const MD5_S: [u32; 64] = [
    7, 12, 17, 22, 7, 12, 17, 22, 7, 12, 17, 22, 7, 12, 17, 22, 5, 9, 14, 20, 5, 9, 14, 20, 5, 9,
    14, 20, 5, 9, 14, 20, 4, 11, 16, 23, 4, 11, 16, 23, 4, 11, 16, 23, 4, 11, 16, 23, 6, 10, 15,
    21, 6, 10, 15, 21, 6, 10, 15, 21, 6, 10, 15, 21,
];

const MD5_K: [u32; 64] = [
    0xd76a_a478,
    0xe8c7_b756,
    0x2420_70db,
    0xc1bd_ceee,
    0xf57c_0faf,
    0x4787_c62a,
    0xa830_4613,
    0xfd46_9501,
    0x6980_98d8,
    0x8b44_f7af,
    0xffff_5bb1,
    0x895c_d7be,
    0x6b90_1122,
    0xfd98_7193,
    0xa679_438e,
    0x49b4_0821,
    0xf61e_2562,
    0xc040_b340,
    0x265e_5a51,
    0xe9b6_c7aa,
    0xd62f_105d,
    0x0244_1453,
    0xd8a1_e681,
    0xe7d3_fbc8,
    0x21e1_cde6,
    0xc337_07d6,
    0xf4d5_0d87,
    0x455a_14ed,
    0xa9e3_e905,
    0xfcef_a3f8,
    0x676f_02d9,
    0x8d2a_4c8a,
    0xfffa_3942,
    0x8771_f681,
    0x6d9d_6122,
    0xfde5_380c,
    0xa4be_ea44,
    0x4bde_cfa9,
    0xf6bb_4b60,
    0xbebf_bc70,
    0x289b_7ec6,
    0xeaa1_27fa,
    0xd4ef_3085,
    0x0488_1d05,
    0xd9d4_d039,
    0xe6db_99e5,
    0x1fa2_7cf8,
    0xc4ac_5665,
    0xf429_2244,
    0x432a_ff97,
    0xab94_23a7,
    0xfc93_a039,
    0x655b_59c3,
    0x8f0c_cc92,
    0xffef_f47d,
    0x8584_5dd1,
    0x6fa8_7e4f,
    0xfe2c_e6e0,
    0xa301_4314,
    0x4e08_11a1,
    0xf753_7e82,
    0xbd3a_f235,
    0x2ad7_d2bb,
    0xeb86_d391,
];

#[derive(Debug, Clone)]
pub struct CloudApi {
    pub region: String,
    pub access_token: String,
    pub refresh_token: String,
    pub user_id: String,
}

impl CloudApi {
    pub fn new(
        region: impl Into<String>,
        access_token: impl Into<String>,
        refresh_token: impl Into<String>,
    ) -> Self {
        Self {
            region: region.into(),
            access_token: access_token.into(),
            refresh_token: refresh_token.into(),
            user_id: String::new(),
        }
    }

    pub fn with_user_id(mut self, user_id: impl Into<String>) -> Self {
        self.user_id = http_user_id(&user_id.into());
        self
    }

    /// iot-service `user-id`: JWT `username`, else `u_{uid}`.
    pub fn auth_user_id(&self) -> String {
        if let Some(id) = jwt_iot_user_id(&self.access_token) {
            return iot_user_id(&id);
        }
        iot_user_id(&self.user_id)
    }

    /// Plugin `bambu_network_get_user_id` is digits; JWT usernames stay `u_{uid}`.
    pub fn ttcode_user_id(&self) -> String {
        if jwt_iot_user_id(&self.access_token).is_some() {
            self.auth_user_id()
        } else {
            http_user_id(&self.user_id)
        }
    }

    fn host(&self) -> &'static str {
        api_host(&self.region)
    }

    #[cfg(test)]
    fn json_headers(&self, json_body: bool) -> Vec<(&str, String)> {
        self.json_headers_as(&self.auth_user_id(), json_body)
    }

    fn json_headers_as(&self, uid: &str, json_body: bool) -> Vec<(&str, String)> {
        let mut h = vec![("Accept", "application/json".into())];
        if json_body {
            h.push(("Content-Type", "application/json".into()));
        }
        if !self.access_token.is_empty() {
            h.push(("Authorization", format!("Bearer {}", self.access_token)));
        }
        if !uid.is_empty() {
            h.push(("user-id", uid.to_string()));
        }
        for (k, v) in slicer_http_headers(&slicer_device_id()) {
            h.push((k, v));
        }
        if let Some(creds) = loaded_slicer_creds() {
            let unix_ms = std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .map(|d| d.as_millis() as u64)
                .unwrap_or(0);
            if let Ok(extra) = http_security_headers(creds, unix_ms) {
                h.extend(extra);
            }
        }
        h
    }

    fn send_json(
        &self,
        method: &str,
        path: &str,
        body: Option<&Value>,
    ) -> Result<Value, CloudApiError> {
        self.send_json_typed(method, path, body, body.is_some(), None)
    }

    fn send_json_typed(
        &self,
        method: &str,
        path: &str,
        body: Option<&Value>,
        json_content_type: bool,
        user_id: Option<&str>,
    ) -> Result<Value, CloudApiError> {
        let payload = body.map(serde_json::to_vec).transpose()?;
        let uid = user_id
            .map(str::to_string)
            .unwrap_or_else(|| self.auth_user_id());
        let owned = self.json_headers_as(&uid, json_content_type);
        let headers: Vec<(&str, &str)> = owned.iter().map(|(k, v)| (*k, v.as_str())).collect();
        tracing::debug!(
            target: "elysian_protocol::cloud",
            method,
            host = self.host(),
            path,
            body_bytes = payload.as_ref().map(Vec::len).unwrap_or(0),
            user_id_len = uid.len(),
            user_id_u_prefix = uid.starts_with("u_"),
            token_len = self.access_token.len(),
            token_jwt_dots = self.access_token.bytes().filter(|&b| b == b'.').count(),
            "cloud HTTPS request"
        );
        let resp = https::request(method, self.host(), path, &headers, payload.as_deref())?;
        let retry_after = resp
            .headers
            .iter()
            .find(|(k, _)| k.eq_ignore_ascii_case("retry-after"))
            .map(|(_, v)| v.as_str());
        let text = resp.body_text();
        tracing::debug!(
            target: "elysian_protocol::cloud",
            method,
            path,
            status = resp.status,
            retry_after,
            body_len = text.len(),
            body_keys = %json_field_names(&text),
            error_code = json_error_code(&text),
            "cloud HTTPS response"
        );
        if resp.status < 200 || resp.status >= 300 {
            return Err(CloudApiError::Message(cloud_http_error(resp.status, &text)));
        }
        if text.trim().is_empty() {
            return Ok(json!({}));
        }
        Ok(serde_json::from_str(&text)?)
    }

    pub fn list_devices(&self) -> Result<Vec<CloudDevice>, CloudApiError> {
        let v = self.send_json("GET", bind_path(), None)?;
        Ok(parse_bind_devices(&v))
    }

    pub fn ttcode(&self, dev_id: &str) -> Result<CameraCreds, CloudApiError> {
        self.mint_camera_creds(dev_id)
    }

    /// Resolve `/bind` serial, then POST ttcode (Studio `dev|fw|proto`).
    pub fn mint_camera_creds(&self, serial: &str) -> Result<CameraCreds, CloudApiError> {
        self.mint_camera_creds_with(serial, None)
    }

    /// `firmware` is MQTT `ota` / Studio `dev_ver` (e.g. `01.02.00.00`).
    pub fn mint_camera_creds_with(
        &self,
        serial: &str,
        firmware: Option<&str>,
    ) -> Result<CameraCreds, CloudApiError> {
        let (serial, bind_fw) = match self.list_devices() {
            Ok(devs) => {
                let serial = camera_serial_from_bind(serial, &devs);
                let bind_fw = devs
                    .iter()
                    .find(|d| d.dev_id == serial)
                    .map(|d| d.dev_ver.trim().to_string())
                    .filter(|v| !v.is_empty());
                tracing::debug!(
                    target: "elysian_protocol::cloud",
                    requested = %redact_id(&serial),
                    device_count = devs.len(),
                    bind_fw_len = bind_fw.as_ref().map(String::len).unwrap_or(0),
                    "bind for camera mint"
                );
                (serial, bind_fw)
            }
            Err(err) => {
                tracing::debug!(
                    target: "elysian_protocol::cloud",
                    error = %err,
                    "bind list failed; using requested serial"
                );
                (serial.trim().to_string(), None)
            }
        };
        let owned = firmware
            .map(str::trim)
            .filter(|v| !v.is_empty())
            .map(str::to_string)
            .or_else(|| {
                std::env::var("BAMBU_DEV_VER")
                    .ok()
                    .map(|s| s.trim().to_string())
                    .filter(|s| !s.is_empty())
            })
            .or(bind_fw);
        let fw = owned.as_deref();
        tracing::debug!(
            target: "elysian_protocol::cloud",
            requested = %redact_id(&serial),
            firmware = fw.unwrap_or(""),
            "mint camera creds"
        );
        // One POST. Extra protocol/user-id retries on iot-service code 8 never
        // succeeded and trip Cloudflare 1015.
        let protocols: &[&str] = if fw.is_some() {
            &["agora"]
        } else {
            &["tutk", "agora"]
        };
        match self.ttcode_with(&serial, fw, protocols) {
            Ok(mut creds) => {
                creds.device = serial;
                Ok(creds)
            }
            Err(err) if !ttcode_should_retry_protocol(&err) => Err(err),
            Err(err) if protocols == ["agora"] => {
                match self.ttcode_with(&serial, fw, &["tutk", "agora"]) {
                    Ok(mut creds) => {
                        creds.device = serial;
                        Ok(creds)
                    }
                    Err(_) => Err(err),
                }
            }
            Err(err) => Err(err),
        }
    }

    /// Studio `NetworkAgent::get_camera_url`: `dev|fw|proto[|channel]`.
    /// Non-403 failures may retry `"agora"`; iot-service 403 is not retried.
    pub fn get_camera_url(&self, key: &str) -> Result<CameraCreds, CloudApiError> {
        let parsed = parse_camera_url_key(key);
        let protocols: Vec<&str> = parsed.protocols.iter().map(String::as_str).collect();
        let first = self.ttcode_post(
            &parsed.dev_id,
            parsed.firmware.as_deref(),
            &protocols,
            parsed.channel.as_deref(),
        );
        match first {
            Ok(creds) => Ok(creds),
            Err(err) if !ttcode_should_retry_protocol(&err) => Err(err),
            Err(err) => {
                let agora_only = protocols.len() != 1 || protocols.first() != Some(&"agora");
                if agora_only {
                    match self.ttcode_post(
                        &parsed.dev_id,
                        parsed.firmware.as_deref(),
                        &["agora"],
                        parsed.channel.as_deref(),
                    ) {
                        Ok(creds) => return Ok(creds),
                        Err(retry) if !ttcode_should_retry_protocol(&retry) => return Err(retry),
                        Err(_) => {}
                    }
                }
                Err(err)
            }
        }
    }

    pub fn ttcode_with(
        &self,
        dev_id: &str,
        firmware: Option<&str>,
        protocols: &[&str],
    ) -> Result<CameraCreds, CloudApiError> {
        self.ttcode_post(dev_id, firmware, protocols, None)
    }

    fn ttcode_post(
        &self,
        dev_id: &str,
        firmware: Option<&str>,
        protocols: &[&str],
        channel: Option<&str>,
    ) -> Result<CameraCreds, CloudApiError> {
        let mut body = ttcode_post_body(dev_id, firmware, protocols);
        if let Some(ch) = channel.filter(|c| !c.is_empty()) {
            body["channel"] = json!(ch);
        }
        let uid = self.ttcode_user_id();
        tracing::debug!(
            target: "elysian_protocol::cloud",
            dev = %redact_id(dev_id),
            firmware = firmware.unwrap_or(""),
            protocols = ?protocols,
            channel_set = channel.map(|c| !c.is_empty()).unwrap_or(false),
            user_id_len = uid.len(),
            user_id_u_prefix = uid.starts_with("u_"),
            "POST ttcode"
        );
        let v =
            self.send_json_typed("POST", ttcode_path(), Some(&body), true, Some(uid.as_str()))?;
        let creds = parse_camera_creds(&v).ok_or_else(|| {
            CloudApiError::Message("ttcode response missing uid/authkey/channel".into())
        })?;
        tracing::debug!(
            target: "elysian_protocol::cloud",
            proto = ?creds.proto,
            has_channel = !creds.channel.is_empty(),
            has_token = !creds.token.is_empty(),
            has_ttcode = !creds.uid.is_empty(),
            token_prefix = %agora_token_kind(&creds.token),
            "ttcode parsed"
        );
        Ok(creds)
    }

    pub fn request_upload(
        &self,
        filename: &str,
        bytes: &[u8],
    ) -> Result<UploadTicket, CloudApiError> {
        let hash = md5_hex(bytes);
        let body = upload_ticket_body(filename, bytes.len(), &hash);
        let v = self.send_json("POST", upload_path(), Some(&body))?;
        parse_upload_ticket(&v)
    }

    pub fn put_file(&self, ticket: &UploadTicket, bytes: &[u8]) -> Result<(), CloudApiError> {
        let mut headers: Vec<(&str, &str)> = vec![("Content-Type", "application/octet-stream")];
        let extra: Vec<(String, String)> = ticket.extra_headers.clone();
        for (k, v) in &extra {
            headers.push((k.as_str(), v.as_str()));
        }
        let resp = https::request_url("PUT", &ticket.put_url, &headers, Some(bytes))?;
        if resp.status < 200 || resp.status >= 300 {
            return Err(CloudApiError::Message(format!(
                "cloud PUT HTTP {}",
                resp.status
            )));
        }
        Ok(())
    }

    pub fn upload_file(&self, filename: &str, bytes: &[u8]) -> Result<UploadTicket, CloudApiError> {
        let ticket = self.request_upload(filename, bytes)?;
        self.put_file(&ticket, bytes)?;
        Ok(ticket)
    }

    pub fn list_filaments(&self) -> Result<Vec<crate::FilamentSpool>, CloudApiError> {
        let v = self.send_json("GET", crate::filament_v2_path(), None)?;
        Ok(crate::parse_cloud_filaments(&v))
    }

    pub fn create_filament(
        &self,
        spool: &crate::FilamentSpool,
    ) -> Result<crate::FilamentSpool, CloudApiError> {
        let body = crate::spool_to_cloud_json(spool);
        let v = self.send_json("POST", crate::filament_v2_path(), Some(&body))?;
        Ok(crate::spool_from_cloud_json(&v)
            .or_else(|| crate::parse_cloud_filaments(&v).into_iter().next())
            .unwrap_or_else(|| {
                let mut next = spool.clone();
                next.cloud_synced = true;
                next
            }))
    }

    pub fn update_filament(
        &self,
        spool: &crate::FilamentSpool,
    ) -> Result<crate::FilamentSpool, CloudApiError> {
        let body = crate::spool_to_cloud_json(spool);
        let v = self.send_json("PUT", crate::filament_v2_path(), Some(&body))?;
        Ok(crate::spool_from_cloud_json(&v).unwrap_or_else(|| {
            let mut next = spool.clone();
            next.cloud_synced = true;
            next
        }))
    }

    pub fn delete_filaments(&self, ids: &[String]) -> Result<(), CloudApiError> {
        let body = crate::batch_delete_body(ids);
        let _ = self.send_json("DELETE", crate::filament_v2_batch_path(), Some(&body))?;
        Ok(())
    }

    pub fn sync_ams_weights(&self, body: &Value) -> Result<Value, CloudApiError> {
        self.send_json("POST", crate::filament_v2_ams_sync_path(), Some(body))
    }

    pub fn login(
        region: &str,
        account: &str,
        password: &str,
        code: Option<&str>,
    ) -> Result<LoginResult, CloudApiError> {
        let api = CloudApi::new(region, String::new(), String::new());
        let v = api.send_json(
            "POST",
            login_path(),
            Some(&login_body(account, password, code)),
        )?;
        parse_login(&v)
    }

    pub fn login_with_ticket(region: &str, ticket: &str) -> Result<LoginResult, CloudApiError> {
        let ticket = ticket.trim();
        if ticket.is_empty() {
            return Err(CloudApiError::Message("empty oauth ticket".into()));
        }
        let api = CloudApi::new(region, String::new(), String::new());
        let v = api.send_json("POST", &ticket_path(ticket), Some(&ticket_body(ticket)))?;
        let parsed = parse_login(&v)?;
        let api = match &parsed {
            LoginResult::Tokens { access_token, .. } => {
                CloudApi::new(region, access_token.clone(), String::new())
            }
            LoginResult::NeedsCode { login_type } => {
                return Err(CloudApiError::Message(format!(
                    "oauth ticket returned loginType {login_type}"
                )));
            }
        };
        let profile = api.profile().ok();
        Ok(enrich_user_id(parsed, profile))
    }

    pub fn profile(&self) -> Result<CloudProfile, CloudApiError> {
        let v = self.send_json("GET", profile_path(), None)?;
        Ok(parse_profile(&v))
    }

    pub fn refresh(&mut self) -> Result<(), CloudApiError> {
        if self.refresh_token.is_empty() {
            return Err(CloudApiError::Message(
                "no cloud_refresh token (sign in again)".into(),
            ));
        }
        let body = refresh_body(&self.refresh_token);
        let v = match self.send_json("POST", refresh_path(), Some(&body)) {
            Ok(v) => v,
            Err(err) if auth_rejected(&err) => {
                let access = self.access_token.clone();
                self.access_token.clear();
                let retry = self.send_json("POST", refresh_path(), Some(&body));
                self.access_token = access;
                retry?
            }
            Err(err) => return Err(err),
        };
        let (access, refresh) = parse_refresh(&v)?;
        self.access_token = access;
        if !refresh.is_empty() {
            self.refresh_token = refresh;
        }
        if self.user_id.is_empty() {
            if let Some(id) = jwt_user_id(&self.access_token) {
                self.user_id = id;
            }
        }
        Ok(())
    }

    pub fn with_retry<T>(
        &mut self,
        mut op: impl FnMut(&CloudApi) -> Result<T, CloudApiError>,
    ) -> Result<T, CloudApiError> {
        if self.user_id.is_empty() {
            if let Some(id) = jwt_user_id(&self.access_token) {
                self.user_id = id;
            }
        }
        match op(self) {
            Err(err) if auth_rejected(&err) => {
                self.refresh().map_err(|refresh_err| {
                    CloudApiError::Message(format!(
                        "{err}; refresh failed ({refresh_err}) — sign in again"
                    ))
                })?;
                op(self)
            }
            other => other,
        }
    }
}

fn auth_rejected(err: &CloudApiError) -> bool {
    err.to_string().contains("HTTP 401")
}

fn json_error_code(text: &str) -> Option<i64> {
    serde_json::from_str::<Value>(text).ok().and_then(|v| {
        v.get("code")
            .and_then(|c| c.as_i64().or_else(|| c.as_u64().map(|n| n as i64)))
            .or_else(|| {
                v.get("code")
                    .and_then(Value::as_str)
                    .and_then(|s| s.parse().ok())
            })
    })
}

fn json_error_hint(text: &str) -> String {
    let parsed = serde_json::from_str::<Value>(text).ok();
    let code = json_error_code(text);
    let msg = parsed
        .as_ref()
        .and_then(|v| {
            v.get("error")
                .and_then(Value::as_str)
                .filter(|s| !s.is_empty())
                .map(str::to_string)
                .or_else(|| {
                    v.get("message")
                        .and_then(Value::as_str)
                        .filter(|s| !s.is_empty())
                        .map(str::to_string)
                })
        })
        .unwrap_or_default();
    match (code, msg.is_empty()) {
        (Some(code), false) => format!("error code {code}: {msg}"),
        (Some(code), true) => format!("error code {code}"),
        (None, false) => msg,
        (None, true) => String::new(),
    }
}

pub fn ttcode_post_body(dev_id: &str, firmware: Option<&str>, protocols: &[&str]) -> Value {
    let mut body = serde_json::Map::new();
    body.insert("dev_id".into(), json!(dev_id));
    if let Some(ver) = firmware.filter(|v| !v.is_empty()) {
        body.insert("dev_ver".into(), json!(ver));
    }
    if !protocols.is_empty() {
        body.insert("protocols".into(), json!(protocols));
    }
    Value::Object(body)
}

/// Studio `X-BBL-Client-Version` / `cli_ver` (installed Bambu Studio).
pub const SLICER_CLIENT_VERSION: &str = "02.08.02.61";
/// Plugin `User-Agent: bambu_network_agent/…` / `X-BBL-Agent-Version`.
pub const SLICER_AGENT_VERSION: &str = "02.08.02.54";

/// Studio `X-BBL-Device-ID` is `slicer_uuid` from BambuStudio.conf, not the printer serial.
pub fn slicer_device_id() -> String {
    static ID: OnceLock<String> = OnceLock::new();
    ID.get_or_init(|| {
        if let Ok(id) = std::env::var("BAMBU_SLICER_UUID") {
            let id = id.trim();
            if !id.is_empty() {
                return id.to_string();
            }
        }
        read_studio_slicer_uuid().unwrap_or_else(|| "bambu-studio-rs".into())
    })
    .clone()
}

fn read_studio_slicer_uuid() -> Option<String> {
    let mut paths = Vec::new();
    if let Ok(xdg) = std::env::var("XDG_CONFIG_HOME") {
        paths.push(PathBuf::from(xdg).join("BambuStudio/BambuStudio.conf"));
    }
    if let Ok(home) = std::env::var("HOME") {
        paths.push(PathBuf::from(home).join(".config/BambuStudio/BambuStudio.conf"));
    }
    for path in paths {
        let Ok(text) = std::fs::read_to_string(path) else {
            continue;
        };
        let Ok(v) = serde_json::from_str::<Value>(&text) else {
            continue;
        };
        if let Some(id) = v.get("slicer_uuid").and_then(Value::as_str) {
            let id = id.trim();
            if !id.is_empty() {
                return Some(id.to_string());
            }
        }
    }
    None
}

pub fn is_cloud_forbidden(err: &CloudApiError) -> bool {
    let t = err.to_string().to_ascii_lowercase();
    t.contains("403") || t.contains("forbidden")
}

pub fn is_cloud_rate_limited(err: &CloudApiError) -> bool {
    cloud_error_is_rate_limited(&err.to_string())
}

pub fn cloud_error_is_rate_limited(text: &str) -> bool {
    let t = text.to_ascii_lowercase();
    t.contains("http 429") || t.contains("1015") || t.contains("rate limit")
}

pub(crate) fn redact_id(id: &str) -> String {
    let id = id.trim();
    if id.len() <= 4 {
        format!("len={}", id.len())
    } else {
        format!("{}…(len={})", &id[..4], id.len())
    }
}

fn json_field_names(text: &str) -> String {
    serde_json::from_str::<Value>(text)
        .ok()
        .and_then(|v| v.as_object().map(|m| m.keys().cloned().collect::<Vec<_>>()))
        .map(|keys| keys.join(","))
        .unwrap_or_default()
}

fn agora_token_kind(token: &str) -> &'static str {
    if token.starts_with("006") {
        "006"
    } else if token.starts_with("007") {
        "007"
    } else if token.is_empty() {
        "empty"
    } else {
        "other"
    }
}

/// Studio `X-BBL-OS-Version` from `wxGetOsVersion` (kernel release on Linux).
pub fn slicer_os_version() -> String {
    std::fs::read_to_string("/proc/sys/kernel/osrelease")
        .ok()
        .map(|s| s.trim().to_string())
        .filter(|s| !s.is_empty())
        .unwrap_or_else(|| "6.1.0".into())
}

fn loaded_slicer_creds() -> Option<&'static SlicerCredentials> {
    static CREDS: OnceLock<SlicerCredentials> = OnceLock::new();
    let creds = CREDS.get_or_init(|| load_from_dir(default_config_dir()).unwrap_or_default());
    creds.has_cert_and_key().then_some(creds)
}

/// Studio `GUI_App::get_extra_header` plus plugin `User-Agent` / agent version.
/// Do not send `x-bbl-be: go` — that is not a Studio extra header and iot-service
/// `/ttcode` returns code 8 when the camera mint is routed to the Go backend.
/// Do not send empty `X-BBL-Executable-info: {}` — the plugin omits it or sends
/// a real blob; `{}` has been observed to 403 `/ttcode` (iot-service code 8).
pub fn slicer_http_headers(device_id: &str) -> Vec<(&'static str, String)> {
    let id = if device_id.is_empty() {
        "bambu-studio-rs"
    } else {
        device_id
    };
    vec![
        (
            "User-Agent",
            format!("bambu_network_agent/{SLICER_AGENT_VERSION}"),
        ),
        ("X-BBL-Client-Type", "slicer".into()),
        ("X-BBL-Client-Name", "BambuStudio".into()),
        ("X-BBL-Client-Version", SLICER_CLIENT_VERSION.into()),
        ("X-BBL-OS-Type", "linux".into()),
        ("X-BBL-OS-Version", slicer_os_version()),
        ("X-BBL-Language", "en-US".into()),
        ("X-BBL-Device-ID", id.into()),
        ("X-BBL-Agent-Version", SLICER_AGENT_VERSION.into()),
        ("X-BBL-Agent-OS-Type", "linux".into()),
    ]
}

/// 403 code 8 / Cloudflare 1015: do not mint again with a different body.
pub fn ttcode_should_retry_protocol(err: &CloudApiError) -> bool {
    !is_cloud_forbidden(err) && !is_cloud_rate_limited(err)
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CameraUrlKey {
    pub dev_id: String,
    pub firmware: Option<String>,
    pub protocols: Vec<String>,
    pub channel: Option<String>,
}

/// Build Studio `dev|fw|"agora"` / `dev|fw|"tutk","agora"|channel`.
pub fn camera_url_key(
    dev_id: &str,
    firmware: &str,
    protocols: &[&str],
    channel: Option<&str>,
) -> String {
    let quoted = protocols
        .iter()
        .map(|p| format!("\"{p}\""))
        .collect::<Vec<_>>()
        .join(",");
    let mut key = format!("{dev_id}|{firmware}|{quoted}");
    if let Some(ch) = channel.filter(|c| !c.is_empty()) {
        key.push('|');
        key.push_str(ch);
    }
    key
}

/// Parse Studio `get_camera_url` / refresh `dev|fw|proto[|channel]`.
pub fn parse_camera_url_key(raw: &str) -> CameraUrlKey {
    let mut parts = raw.split('|');
    let dev_id = parts.next().unwrap_or("").trim().to_string();
    let firmware = parts
        .next()
        .map(str::trim)
        .filter(|s| !s.is_empty())
        .map(str::to_string);
    let proto_raw = parts.next().unwrap_or("").trim();
    let mut protocols: Vec<String> = proto_raw
        .split(',')
        .map(|s| s.trim().trim_matches('"').to_string())
        .filter(|s| !s.is_empty())
        .collect();
    if protocols.is_empty() {
        protocols.extend(["tutk".into(), "agora".into()]);
    }
    let channel = parts
        .next()
        .map(str::trim)
        .filter(|s| !s.is_empty())
        .map(str::to_string);
    CameraUrlKey {
        dev_id,
        firmware,
        protocols,
        channel,
    }
}

pub fn ttcode_get_path(dev_id: &str) -> String {
    format!(
        "{}?dev_id={}",
        ttcode_path(),
        crate::oauth::percent_encode_query(dev_id)
    )
}

/// GET `/ttcode` is not a Studio method (the public API is POST-only).
pub fn ttcode_should_retry_get(err: &CloudApiError) -> bool {
    let _ = err;
    false
}

pub fn cloud_http_error(status: u16, body: &str) -> String {
    if status == 429 || body.contains("1015") {
        return format!(
            "cloud HTTP {status} (Cloudflare rate limit 1015). Wait a minute before Play; extra /ttcode retries make this worse."
        );
    }
    let hint = json_error_hint(body);
    if !hint.is_empty() {
        return format!("cloud HTTP {status}: {hint}");
    }
    if status == 401 {
        return format!("cloud HTTP {status} (token expired or rejected)");
    }
    let snippet = body.trim();
    if snippet.is_empty() {
        format!("cloud HTTP {status}")
    } else {
        let clipped: String = snippet.chars().take(180).collect();
        format!("cloud HTTP {status}: {clipped}")
    }
}

/// Resolve ttcode JSON after POST. GET is not a valid fallback (405).
pub fn ttcode_after_post(post: Result<Value, CloudApiError>) -> Result<CameraCreds, CloudApiError> {
    parse_camera_creds(&post?)
        .ok_or_else(|| CloudApiError::Message("ttcode response missing uid/authkey".into()))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn md5_matches_rfc() {
        assert_eq!(md5_hex(b""), "d41d8cd98f00b204e9800998ecf8427e");
        assert_eq!(md5_hex(b"abc"), "900150983cd24fb0d6963f7d28e17f72");
    }

    #[test]
    fn bind_fixture_devices() {
        let v = serde_json::json!({
            "code": 0,
            "data": {
                "bind_list": [
                    {
                        "dev_id": "01P00AFAKE00001",
                        "name": "Workbench",
                        "dev_name": "P1S",
                        "online": true
                    }
                ]
            }
        });
        let devices = parse_bind_devices(&v);
        assert_eq!(devices.len(), 1);
        assert_eq!(devices[0].dev_id, "01P00AFAKE00001");
        assert!(devices[0].online);
        assert!(devices[0].access_code.is_empty());
        assert!(devices[0].label().contains("Workbench"));
    }

    #[test]
    fn bind_parses_dev_access_code() {
        let v = serde_json::json!({
            "devices": [{
                "dev_id": "01H2C0000000001",
                "name": "H2C",
                "dev_product_name": "H2C",
                "online": true,
                "dev_access_code": "abcd1234",
                "dev_ver": "01.02.00.00"
            }]
        });
        let devices = parse_bind_devices(&v);
        assert_eq!(devices[0].access_code, "abcd1234");
        assert_eq!(devices[0].dev_ver, "01.02.00.00");
        assert_eq!(
            camera_serial_from_bind("01P00AFAKE00001", &devices),
            "01H2C0000000001"
        );
        assert_eq!(
            camera_serial_from_bind("01H2C0000000001", &devices),
            "01H2C0000000001"
        );
    }

    #[test]
    fn bind_data_devices_shape() {
        let v = serde_json::json!({
            "data": {
                "devices": [
                    {
                        "dev_id": "01P00AFAKE00003",
                        "name": "Office",
                        "dev_name": "A1",
                        "online": "1"
                    }
                ]
            }
        });
        let devices = parse_bind_devices(&v);
        assert_eq!(devices.len(), 1);
        assert_eq!(devices[0].dev_id, "01P00AFAKE00003");
        assert!(devices[0].online);
    }

    #[test]
    fn upload_ticket_and_body() {
        let body = upload_ticket_body("cube.gcode.3mf", 12, "d41d8cd98f00b204e9800998ecf8427e");
        assert_eq!(body["filename"], "cube.gcode.3mf");
        assert_eq!(body["size"], 12);
        let v = serde_json::json!({
            "data": {
                "url": "https://oss.example/put",
                "public_url": "https://cdn.example/cube.gcode.3mf"
            }
        });
        let ticket = parse_upload_ticket(&v).unwrap();
        assert_eq!(ticket.put_url, "https://oss.example/put");
        assert!(ticket.public_url.starts_with("https://"));
    }

    #[test]
    fn upload_ticket_nested_url_and_headers() {
        let v = serde_json::json!({
            "data": {
                "url": {
                    "put": "https://oss.example/put",
                    "get": "https://cdn.example/cube.gcode.3mf"
                },
                "headers": {
                    "x-amz-acl": "private"
                }
            }
        });
        let ticket = parse_upload_ticket(&v).unwrap();
        assert_eq!(ticket.put_url, "https://oss.example/put");
        assert_eq!(ticket.public_url, "https://cdn.example/cube.gcode.3mf");
        assert!(ticket
            .extra_headers
            .iter()
            .any(|(k, v)| k == "x-amz-acl" && v == "private"));
    }

    #[test]
    fn parse_refresh_tokens() {
        let v = serde_json::json!({
            "data": {
                "accessToken": "tok_refresh",
                "refreshToken": "ref_refresh",
                "userId": "99"
            }
        });
        let (access, refresh) = parse_refresh(&v).unwrap();
        assert_eq!(access, "tok_refresh");
        assert_eq!(refresh, "ref_refresh");
    }

    #[test]
    fn login_needs_email_code() {
        let v = serde_json::json!({
            "data": { "loginType": "verifyCode" }
        });
        match parse_login(&v).unwrap() {
            LoginResult::NeedsCode { login_type } => assert_eq!(login_type, "verifyCode"),
            LoginResult::Tokens { .. } => panic!("expected code prompt"),
        }
    }

    #[test]
    fn login_tokens() {
        let v = serde_json::json!({
            "accessToken": "tok_test",
            "refreshToken": "ref_test",
            "userId": "4242"
        });
        match parse_login(&v).unwrap() {
            LoginResult::Tokens {
                access_token,
                user_id,
                ..
            } => {
                assert_eq!(access_token, "tok_test");
                assert_eq!(user_id, "4242");
            }
            LoginResult::NeedsCode { .. } => panic!("expected tokens"),
        }
    }

    #[test]
    fn ticket_login_fixture() {
        let v = serde_json::json!({
            "accessToken": "tok_ticket",
            "refreshToken": "ref_ticket",
            "accessMethod": "ticket",
            "loginType": ""
        });
        match parse_login(&v).unwrap() {
            LoginResult::Tokens { access_token, .. } => assert_eq!(access_token, "tok_ticket"),
            LoginResult::NeedsCode { .. } => panic!("expected tokens"),
        }
    }

    #[test]
    fn profile_uid_str() {
        let v = serde_json::json!({
            "uidStr": "4242",
            "name": "Ada",
            "account": "ada@example.com"
        });
        let p = parse_profile(&v);
        assert_eq!(p.user_id, "4242");
        assert_eq!(p.account, "ada@example.com");
    }

    #[test]
    fn jwt_uid_from_payload() {
        // {"uid":"99"}
        let payload = base64::Engine::encode(
            &base64::engine::general_purpose::URL_SAFE_NO_PAD,
            br#"{"uid":"99"}"#,
        );
        let token = format!("eyJhbGciOiJub25lIn0.{payload}.sig");
        assert_eq!(jwt_user_id(&token).as_deref(), Some("99"));
    }

    #[test]
    fn jwt_user_id_field_and_mqtt_prefix() {
        let payload = base64::Engine::encode(
            &base64::engine::general_purpose::URL_SAFE_NO_PAD,
            br#"{"user_id":"012345678"}"#,
        );
        let token = format!("eyJhbGciOiJub25lIn0.{payload}.sig");
        assert_eq!(jwt_user_id(&token).as_deref(), Some("012345678"));
        assert_eq!(http_user_id("u_4242"), "4242");
        assert_eq!(iot_user_id("4242"), "u_4242");
        assert_eq!(iot_user_id("u_4242"), "u_4242");
        let api = CloudApi::new("us", token, String::new());
        assert_eq!(api.auth_user_id(), "u_012345678");
        let api = CloudApi::new("us", "tok", String::new()).with_user_id("u_99");
        assert_eq!(api.auth_user_id(), "u_99");
    }

    #[test]
    fn ttcode_user_id_prefers_jwt_username() {
        let payload = base64::Engine::encode(
            &base64::engine::general_purpose::URL_SAFE_NO_PAD,
            br#"{"username":"u_4242","user_id":"4242"}"#,
        );
        let token = format!("eyJhbGciOiJub25lIn0.{payload}.sig");
        assert_eq!(jwt_iot_user_id(&token).as_deref(), Some("u_4242"));
        let api = CloudApi::new("us", token, String::new()).with_user_id("999");
        assert_eq!(api.auth_user_id(), "u_4242");
        assert_eq!(api.ttcode_user_id(), "u_4242");
    }

    #[test]
    fn ttcode_user_id_is_digits_for_studio_token() {
        let api = CloudApi::new("us", "tok", String::new()).with_user_id("u_4242");
        assert_eq!(api.auth_user_id(), "u_4242");
        assert_eq!(api.ttcode_user_id(), "4242");
    }

    #[test]
    fn json_error_hint_skips_empty_message() {
        let hint = json_error_hint(
            r#"{"code":8,"error":"The specified resource is forbidden.","message":""}"#,
        );
        assert!(hint.contains("forbidden"));
        assert!(hint.contains("error code 8"));
        let err = CloudApiError::Message(cloud_http_error(
            403,
            r#"{"code":8,"error":"The specified resource is forbidden.","message":""}"#,
        ));
        assert!(is_cloud_forbidden(&err));
    }

    #[test]
    fn region_hosts() {
        assert_eq!(api_host("us"), API_HOST_US);
        assert_eq!(api_host("CN"), API_HOST_CN);
    }

    #[test]
    fn ttcode_fixture_tutk_url() {
        let v = serde_json::json!({
            "message": "success",
            "ttcode": "01234567890ABCDEF012",
            "authkey": "01234567",
            "passwd": "012345",
            "region": "us",
            "type": "tutk"
        });
        let creds = parse_camera_creds(&v).unwrap();
        assert_eq!(creds.proto, CameraProto::Tutk);
        assert!(creds
            .bambu_url()
            .starts_with("bambu:///tutk?uid=01234567890ABCDEF012"));
    }

    #[test]
    fn ttcode_fixture_agora_url() {
        let v = serde_json::json!({
            "data": {
                "type": "agora",
                "channel_name": "devchan",
                "region": "us",
                "token": "tok",
                "authkey": "ak",
                "app_id": "app",
                "stream_key": "k",
                "stream_salt": "s",
                "user": "42"
            }
        });
        let creds = parse_camera_creds(&v).unwrap();
        assert_eq!(creds.proto, CameraProto::Agora);
        let url = creds.bambu_url();
        assert!(url.starts_with("bambu:///agora?channel=devchan"));
        assert!(url.contains("token=tok"));
        assert!(url.contains("streamKey=k"));
        assert!(url.contains("streamSalt=s"));
        assert!(url.contains("user=42"));
        let mut with_dev = creds.clone();
        with_dev.device = "01P00A000000001".into();
        assert!(with_dev.bambu_url().contains("&device=01P00A000000001"));
        assert!(creds
            .bambu_url_for_device("01P00A000000001")
            .contains("&device=01P00A000000001"));
    }

    #[test]
    fn camera_url_key_roundtrip_studio_quotes() {
        let key = camera_url_key("01H2C", "01.00", &["tutk", "agora"], Some("ch"));
        let parsed = parse_camera_url_key(&key);
        assert_eq!(parsed.dev_id, "01H2C");
        assert_eq!(parsed.firmware.as_deref(), Some("01.00"));
        assert_eq!(parsed.protocols, vec!["tutk", "agora"]);
        assert_eq!(parsed.channel.as_deref(), Some("ch"));
        let body = ttcode_post_body(
            &parsed.dev_id,
            parsed.firmware.as_deref(),
            &parsed
                .protocols
                .iter()
                .map(String::as_str)
                .collect::<Vec<_>>(),
        );
        assert_eq!(body["dev_id"], "01H2C");
        assert_eq!(body["protocols"][1], "agora");
    }

    #[test]
    fn ttcode_post_body_is_studio_shaped() {
        let v = ttcode_post_body("01P00A000000001", Some("01.07.00.00"), &["tutk", "agora"]);
        assert_eq!(v["dev_id"], "01P00A000000001");
        assert_eq!(v["dev_ver"], "01.07.00.00");
        assert_eq!(
            v["protocols"]
                .as_array()
                .map(|a| a.iter().filter_map(Value::as_str).collect::<Vec<_>>()),
            Some(vec!["tutk", "agora"])
        );
    }

    #[test]
    fn ttcode_get_omits_content_type() {
        let path = ttcode_get_path("01P 1");
        assert!(path.starts_with("/v1/iot-service/api/user/ttcode?dev_id="));
        assert!(path.contains("01P"));
        let api = CloudApi::new("us", "tok", String::new()).with_user_id("4242");
        let get = api.json_headers(false);
        assert!(!get.iter().any(|(k, _)| *k == "Content-Type"));
        assert!(get.iter().any(|(k, _)| *k == "Authorization"));
        assert!(get.iter().any(|(k, v)| *k == "user-id" && v == "u_4242"));
        assert!(get
            .iter()
            .any(|(k, v)| *k == "X-BBL-Client-Type" && v == "slicer"));
        assert!(get
            .iter()
            .any(|(k, v)| *k == "X-BBL-Client-Name" && v == "BambuStudio"));
        assert!(get.iter().any(|(k, _)| *k == "X-BBL-OS-Version"));
        assert!(get
            .iter()
            .any(|(k, v)| *k == "X-BBL-Language" && v == "en-US"));
        assert!(!get
            .iter()
            .any(|(k, _)| k.eq_ignore_ascii_case("x-bbl-executable-info")));
        assert!(!get.iter().any(|(k, _)| k.eq_ignore_ascii_case("x-bbl-be")));
        assert!(!get
            .iter()
            .any(|(k, _)| k.eq_ignore_ascii_case("x-bbl-client-country")));
        assert!(get
            .iter()
            .any(|(k, v)| *k == "User-Agent" && v.starts_with("bambu_network_agent/")));
        let post = api.json_headers(true);
        assert!(post.iter().any(|(k, _)| *k == "Content-Type"));
    }

    #[test]
    fn ttcode_post_success_parses_tutk_url() {
        let get = serde_json::json!({
            "message": "success",
            "ttcode": "01234567890ABCDEF012",
            "authkey": "01234567",
            "passwd": "012345",
            "region": "us",
            "type": "tutk"
        });
        let creds = ttcode_after_post(Ok(get)).unwrap();
        assert_eq!(creds.proto, CameraProto::Tutk);
        assert!(creds
            .bambu_url()
            .starts_with("bambu:///tutk?uid=01234567890ABCDEF012"));
        let forbidden = CloudApiError::Message(cloud_http_error(
            403,
            r#"{"code":8,"error":"The specified resource is forbidden.","message":""}"#,
        ));
        assert!(!ttcode_should_retry_get(&forbidden));
        assert!(forbidden.to_string().contains("forbidden"));
        let method = CloudApiError::Message(cloud_http_error(405, ""));
        assert!(!ttcode_should_retry_get(&method));
        assert!(ttcode_after_post(Err(method)).is_err());
    }

    #[test]
    fn cloud_http_error_includes_405_body() {
        let msg = cloud_http_error(405, "Method Not Allowed");
        assert!(msg.contains("405"));
        assert!(msg.contains("Method Not Allowed"));
    }

    #[test]
    fn cloud_http_error_names_cloudflare_1015() {
        let msg = cloud_http_error(429, "error code: 1015");
        assert!(msg.contains("429"));
        assert!(msg.contains("rate limit"));
        assert!(msg.contains("1015"));
        assert!(cloud_error_is_rate_limited(&msg));
        let err = CloudApiError::Message(msg);
        assert!(is_cloud_rate_limited(&err));
        let forbidden = CloudApiError::Message(cloud_http_error(
            403,
            r#"{"code":8,"error":"The specified resource is forbidden."}"#,
        ));
        assert!(!is_cloud_rate_limited(&forbidden));
        assert!(!cloud_error_is_rate_limited(&forbidden.to_string()));
        assert!(!ttcode_should_retry_protocol(&forbidden));
        let other = CloudApiError::Message("ttcode response missing uid".into());
        assert!(ttcode_should_retry_protocol(&other));
    }

    #[test]
    fn redact_id_keeps_prefix_not_full_serial() {
        let shown = redact_id("31B800000000001");
        assert!(shown.starts_with("31B8"));
        assert!(shown.contains("len=15"));
        assert!(!shown.contains("00000000001"));
    }
}
