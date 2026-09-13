//! Public Bambu cloud HTTPS (OpenBambuAPI / Home Assistant Bambu Lab).
//!
//! Fixture-tested JSON only in `cargo test --offline`. Live calls stay in CLI/UI.

use serde_json::{json, Value};
use thiserror::Error;

use crate::https::{self, HttpsError};

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
}

#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct CloudDevice {
    pub dev_id: String,
    pub name: String,
    pub online: bool,
    pub dev_name: String,
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

pub fn login_path() -> &'static str {
    "/v1/user-service/user/login"
}

pub fn refresh_path() -> &'static str {
    "/v1/user-service/user/refreshtoken"
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
    Some(CloudDevice {
        dev_id,
        name,
        online,
        dev_name,
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
        }
    }

    fn host(&self) -> &'static str {
        api_host(&self.region)
    }

    fn json_headers(&self) -> Vec<(&str, String)> {
        let mut h = vec![
            ("Accept", "application/json".into()),
            ("Content-Type", "application/json".into()),
        ];
        if !self.access_token.is_empty() {
            h.push(("Authorization", format!("Bearer {}", self.access_token)));
        }
        h
    }

    fn send_json(
        &self,
        method: &str,
        path: &str,
        body: Option<&Value>,
    ) -> Result<Value, CloudApiError> {
        let payload = body.map(serde_json::to_vec).transpose()?;
        let owned = self.json_headers();
        let headers: Vec<(&str, &str)> = owned.iter().map(|(k, v)| (*k, v.as_str())).collect();
        let resp = https::request(method, self.host(), path, &headers, payload.as_deref())?;
        if resp.status == 401 || resp.status == 403 {
            return Err(CloudApiError::Message(format!(
                "cloud HTTP {} (token expired or rejected)",
                resp.status
            )));
        }
        if resp.status < 200 || resp.status >= 300 {
            return Err(CloudApiError::Message(format!(
                "cloud HTTP {}",
                resp.status
            )));
        }
        let text = resp.body_text();
        if text.trim().is_empty() {
            return Ok(json!({}));
        }
        Ok(serde_json::from_str(&text)?)
    }

    pub fn list_devices(&self) -> Result<Vec<CloudDevice>, CloudApiError> {
        let v = self.send_json("GET", bind_path(), None)?;
        Ok(parse_bind_devices(&v))
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

    pub fn refresh(&mut self) -> Result<(), CloudApiError> {
        if self.refresh_token.is_empty() {
            return Err(CloudApiError::Message("no cloud_refresh token".into()));
        }
        let v = self.send_json(
            "POST",
            refresh_path(),
            Some(&refresh_body(&self.refresh_token)),
        )?;
        let (access, refresh) = parse_refresh(&v)?;
        self.access_token = access;
        if !refresh.is_empty() {
            self.refresh_token = refresh;
        }
        Ok(())
    }

    pub fn with_retry<T>(
        &mut self,
        mut op: impl FnMut(&CloudApi) -> Result<T, CloudApiError>,
    ) -> Result<T, CloudApiError> {
        match op(self) {
            Err(CloudApiError::Message(msg))
                if msg.contains("HTTP 401") || msg.contains("HTTP 403") =>
            {
                self.refresh()?;
                op(self)
            }
            other => other,
        }
    }
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
        assert!(devices[0].label().contains("Workbench"));
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
    fn region_hosts() {
        assert_eq!(api_host("us"), API_HOST_US);
        assert_eq!(api_host("CN"), API_HOST_CN);
    }
}
