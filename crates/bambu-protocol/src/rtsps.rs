//! X1/H2 LAN live view: RTSPS `:322` interleaved RTP H.264 → RGBA.
//!
//! Cloud Agora stays in `tutk` / `cloud_api`: this tree does not dlopen
//! `libbambu_networking` or `agora_rtc_sdk`.

use std::io::{Read, Write};
use std::time::Duration;

use bambu_device::Frame;
use base64::Engine;
use openh264::formats::YUVSource;
use rustls::{ClientConnection, StreamOwned};

use crate::camera::{tls_connect, CameraError, RtspsSession, LAN_RTSPS_PORT};
use crate::cloud_api::md5_hex;
use crate::mqtt::LAN_MQTT_USER;

const STREAM_PATH: &str = "/streaming/live/1";
const MAX_BUF: usize = 2 * 1024 * 1024;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum VideoCodec {
    H264,
    Hevc,
    Other,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct SdpVideo {
    pub codec: VideoCodec,
    pub payload: u8,
    pub control: String,
}

struct DigestChallenge {
    realm: String,
    nonce: String,
}

enum Auth {
    Basic,
    Digest(DigestChallenge),
}

struct RtspConn {
    tls: StreamOwned<ClientConnection, std::net::TcpStream>,
    buf: Vec<u8>,
    cseq: u32,
    auth: Auth,
    user: String,
    password: String,
}

pub(crate) fn describe(host: &str, access_code: &str) -> Result<RtspsSession, CameraError> {
    let mut conn = RtspConn::connect(host, access_code)?;
    let base = rtsp_base(host);
    let (sdp, _) = conn.describe(&base)?;
    Ok(RtspsSession {
        url: format!("RTSPS :{LAN_RTSPS_PORT} on {host}"),
        sdp,
    })
}

pub(crate) fn stream_frames(
    host: &str,
    access_code: &str,
    mut on_frame: impl FnMut(Frame) -> bool,
) -> Result<(), CameraError> {
    let mut conn = RtspConn::connect(host, access_code)?;
    let base = rtsp_base(host);
    let (sdp, content_base) = conn.describe(&base)?;
    let video = parse_sdp_video(&sdp, &content_base)
        .ok_or_else(|| CameraError::Message("RTSPS DESCRIBE has no video track".into()))?;
    if video.codec == VideoCodec::Hevc {
        return Err(CameraError::Message(
            "RTSPS :322 is HEVC; this client decodes H.264 only".into(),
        ));
    }
    if video.codec != VideoCodec::H264 {
        return Err(CameraError::Message("RTSPS :322 video is not H.264".into()));
    }
    let session = match conn.setup(&video.control) {
        Ok(session) => session,
        Err(err) => {
            let alt = format!("{}/trackID=1", content_base.trim_end_matches('/'));
            conn.setup(&alt).map_err(|_| err)?
        }
    };
    conn.play(&base, &session)?;
    conn.tls
        .get_ref()
        .set_read_timeout(Some(Duration::from_secs(20)))?;
    let mut decoder =
        openh264::decoder::Decoder::new().map_err(|err| CameraError::Message(err.to_string()))?;
    let mut depay = H264Depay::default();
    loop {
        let (channel, packet) = conn.read_interleaved()?;
        if channel != 0 {
            continue;
        }
        if packet.len() < 2 || (packet[1] & 0x7f) != video.payload {
            continue;
        }
        let Some(payload) = rtp_payload(&packet) else {
            continue;
        };
        for nal in depay.push(payload) {
            match decoder.decode(&nal) {
                Ok(Some(yuv)) => {
                    let (width, height) = yuv.dimensions();
                    let mut rgba = vec![0u8; yuv.rgba8_len()];
                    yuv.write_rgba8(&mut rgba);
                    let frame = Frame {
                        width: width as u32,
                        height: height as u32,
                        rgba,
                    };
                    if !on_frame(frame) {
                        return Ok(());
                    }
                }
                Ok(None) => {}
                Err(_) => {}
            }
        }
    }
}

fn rtsp_base(host: &str) -> String {
    format!("rtsp://{host}:{LAN_RTSPS_PORT}{STREAM_PATH}")
}

impl RtspConn {
    fn connect(host: &str, access_code: &str) -> Result<Self, CameraError> {
        if access_code.is_empty() {
            return Err(CameraError::Message("LAN access code is empty".into()));
        }
        let tls = tls_connect(host, LAN_RTSPS_PORT)?;
        Ok(Self {
            tls,
            buf: Vec::new(),
            cseq: 1,
            auth: Auth::Basic,
            user: LAN_MQTT_USER.to_string(),
            password: access_code.to_string(),
        })
    }

    fn authorization(&self, method: &str, uri: &str) -> String {
        match &self.auth {
            Auth::Basic => basic_auth(&self.user, &self.password),
            Auth::Digest(challenge) => {
                digest_auth(&self.user, &self.password, challenge, method, uri)
            }
        }
    }

    fn describe(&mut self, base: &str) -> Result<(String, String), CameraError> {
        let _ = self.roundtrip("OPTIONS", base, &[("User-Agent", "bambu-studio-rs")])?;
        let (status, headers, body) = self.roundtrip(
            "DESCRIBE",
            base,
            &[
                ("Accept", "application/sdp"),
                ("User-Agent", "bambu-studio-rs"),
            ],
        )?;
        if status != 200 {
            return Err(CameraError::Message(format!(
                "RTSPS DESCRIBE HTTP {status}"
            )));
        }
        if body.trim().is_empty() {
            return Err(CameraError::Message("empty RTSPS DESCRIBE body".into()));
        }
        let content_base = headers
            .get("content-base")
            .map(|s| s.trim().trim_end_matches('/').to_string())
            .filter(|s| !s.is_empty())
            .unwrap_or_else(|| base.trim_end_matches('/').to_string());
        Ok((body, content_base))
    }

    fn setup(&mut self, control: &str) -> Result<String, CameraError> {
        let (status, headers, _) = self.roundtrip(
            "SETUP",
            control,
            &[
                ("Transport", "RTP/AVP/TCP;unicast;interleaved=0-1"),
                ("User-Agent", "bambu-studio-rs"),
            ],
        )?;
        if status != 200 {
            return Err(CameraError::Message(format!("RTSPS SETUP HTTP {status}")));
        }
        parse_session(headers.get("session").map(String::as_str).unwrap_or(""))
            .ok_or_else(|| CameraError::Message("RTSPS SETUP missing Session".into()))
    }

    fn play(&mut self, base: &str, session: &str) -> Result<(), CameraError> {
        let (status, _, _) = self.roundtrip(
            "PLAY",
            base,
            &[
                ("Session", session),
                ("Range", "npt=0.000-"),
                ("User-Agent", "bambu-studio-rs"),
            ],
        )?;
        if status != 200 {
            return Err(CameraError::Message(format!("RTSPS PLAY HTTP {status}")));
        }
        Ok(())
    }

    fn roundtrip(
        &mut self,
        method: &str,
        url: &str,
        extra: &[(&str, &str)],
    ) -> Result<(u16, std::collections::BTreeMap<String, String>, String), CameraError> {
        let mut last = self.send(method, url, extra)?;
        if last.0 == 401 {
            if let Some(challenge) =
                parse_digest_challenge(last.1.get("www-authenticate").unwrap_or(&String::new()))
            {
                self.auth = Auth::Digest(challenge);
                last = self.send(method, url, extra)?;
            }
        }
        Ok(last)
    }

    fn send(
        &mut self,
        method: &str,
        url: &str,
        extra: &[(&str, &str)],
    ) -> Result<(u16, std::collections::BTreeMap<String, String>, String), CameraError> {
        let auth = self.authorization(method, url);
        let req = build_request(method, url, self.cseq, &auth, extra);
        self.cseq += 1;
        self.tls.write_all(req.as_bytes())?;
        self.tls.flush()?;
        self.read_response()
    }

    fn fill(&mut self) -> Result<(), CameraError> {
        if self.buf.len() >= MAX_BUF {
            return Err(CameraError::Message("RTSPS buffer overflow".into()));
        }
        let mut tmp = [0u8; 4096];
        let n = self.tls.read(&mut tmp)?;
        if n == 0 {
            return Err(CameraError::Message("RTSPS connection closed".into()));
        }
        self.buf.extend_from_slice(&tmp[..n]);
        Ok(())
    }

    fn read_response(
        &mut self,
    ) -> Result<(u16, std::collections::BTreeMap<String, String>, String), CameraError> {
        loop {
            if let Some(split) = find_headers_end(&self.buf) {
                let head = String::from_utf8_lossy(&self.buf[..split]).into_owned();
                let headers = parse_headers(&head);
                let status = rtsp_status(&head)?;
                let len = headers
                    .get("content-length")
                    .and_then(|v| v.parse::<usize>().ok())
                    .unwrap_or(0);
                while self.buf.len() < split + len {
                    self.fill()?;
                }
                let body = self.buf[split..split + len].to_vec();
                self.buf.drain(..split + len);
                return Ok((status, headers, String::from_utf8_lossy(&body).into_owned()));
            }
            self.fill()?;
        }
    }

    fn read_interleaved(&mut self) -> Result<(u8, Vec<u8>), CameraError> {
        loop {
            if self.buf.first() == Some(&b'$') {
                if self.buf.len() >= 4 {
                    let channel = self.buf[1];
                    let len = u16::from_be_bytes([self.buf[2], self.buf[3]]) as usize;
                    if self.buf.len() >= 4 + len {
                        let packet = self.buf[4..4 + len].to_vec();
                        self.buf.drain(..4 + len);
                        return Ok((channel, packet));
                    }
                }
            } else if let Some(split) = find_headers_end(&self.buf) {
                // Late RTSP (keepalive / error); drop it.
                let head = String::from_utf8_lossy(&self.buf[..split]).into_owned();
                let headers = parse_headers(&head);
                let len = headers
                    .get("content-length")
                    .and_then(|v| v.parse::<usize>().ok())
                    .unwrap_or(0);
                while self.buf.len() < split + len {
                    self.fill()?;
                }
                self.buf.drain(..split + len);
                continue;
            } else if !self.buf.is_empty() && self.buf[0] != b'$' && self.buf[0] != b'R' {
                self.buf.remove(0);
                continue;
            }
            self.fill()?;
        }
    }
}

fn build_request(
    method: &str,
    url: &str,
    cseq: u32,
    authorization: &str,
    extra: &[(&str, &str)],
) -> String {
    let mut req = format!("{method} {url} RTSP/1.0\r\nCSeq: {cseq}\r\n");
    if !authorization.is_empty() {
        req.push_str("Authorization: ");
        req.push_str(authorization);
        req.push_str("\r\n");
    }
    for (key, value) in extra {
        req.push_str(key);
        req.push_str(": ");
        req.push_str(value);
        req.push_str("\r\n");
    }
    req.push_str("\r\n");
    req
}

fn basic_auth(user: &str, password: &str) -> String {
    let raw = format!("{user}:{password}");
    format!(
        "Basic {}",
        base64::engine::general_purpose::STANDARD.encode(raw.as_bytes())
    )
}

fn digest_auth(
    user: &str,
    password: &str,
    challenge: &DigestChallenge,
    method: &str,
    uri: &str,
) -> String {
    let ha1 = md5_hex(format!("{user}:{}:{password}", challenge.realm).as_bytes());
    let ha2 = md5_hex(format!("{method}:{uri}").as_bytes());
    let response = md5_hex(format!("{ha1}:{}:{ha2}", challenge.nonce).as_bytes());
    format!(
        "Digest username=\"{user}\", realm=\"{}\", nonce=\"{}\", uri=\"{uri}\", response=\"{response}\"",
        challenge.realm, challenge.nonce
    )
}

fn parse_digest_challenge(header: &str) -> Option<DigestChallenge> {
    let lower = header.to_ascii_lowercase();
    if !lower.contains("digest") {
        return None;
    }
    Some(DigestChallenge {
        realm: quoted_param(header, "realm")?,
        nonce: quoted_param(header, "nonce")?,
    })
}

fn quoted_param(header: &str, name: &str) -> Option<String> {
    let needle = format!("{name}=");
    let lower = header.to_ascii_lowercase();
    let start = lower.find(&needle)?;
    let rest = &header[start + needle.len()..];
    if let Some(stripped) = rest.strip_prefix('"') {
        let end = stripped.find('"')?;
        Some(stripped[..end].to_string())
    } else {
        let end = rest.find([',', ' ']).unwrap_or(rest.len());
        Some(rest[..end].trim().to_string())
    }
}

fn find_headers_end(buf: &[u8]) -> Option<usize> {
    buf.windows(4).position(|w| w == b"\r\n\r\n").map(|i| i + 4)
}

fn rtsp_status(head: &str) -> Result<u16, CameraError> {
    let line = head.lines().next().unwrap_or("");
    let mut parts = line.split_whitespace();
    let _ = parts.next();
    parts
        .next()
        .and_then(|s| s.parse().ok())
        .ok_or_else(|| CameraError::Message(format!("bad RTSP status: {line}")))
}

fn parse_headers(head: &str) -> std::collections::BTreeMap<String, String> {
    let mut out = std::collections::BTreeMap::new();
    for line in head.lines().skip(1) {
        let Some((key, value)) = line.split_once(':') else {
            continue;
        };
        out.insert(key.trim().to_ascii_lowercase(), value.trim().to_string());
    }
    out
}

fn parse_session(raw: &str) -> Option<String> {
    let token = raw.split(';').next()?.trim();
    if token.is_empty() {
        None
    } else {
        Some(token.to_string())
    }
}

pub(crate) fn parse_sdp_video(sdp: &str, base: &str) -> Option<SdpVideo> {
    let mut in_video = false;
    let mut payload = 96u8;
    let mut codec = VideoCodec::Other;
    let mut control = String::new();
    let mut content_base = base.to_string();
    for line in sdp.lines() {
        let line = line.trim();
        if let Some(rest) = line.strip_prefix("Content-Base:") {
            content_base = rest.trim().trim_end_matches('/').to_string();
        }
        if line.starts_with("m=video") {
            in_video = true;
            if let Some(pt) = line.split_whitespace().nth(3) {
                payload = pt.parse().unwrap_or(96);
            }
            continue;
        }
        if line.starts_with("m=") {
            in_video = false;
            continue;
        }
        if !in_video {
            continue;
        }
        if let Some(rest) = line.strip_prefix("a=rtpmap:") {
            let mut parts = rest.split_whitespace();
            if let Some(pt) = parts.next().and_then(|s| s.parse().ok()) {
                payload = pt;
            }
            let name = parts.next().unwrap_or("").to_ascii_uppercase();
            codec = if name.starts_with("H264") {
                VideoCodec::H264
            } else if name.starts_with("H265") || name.starts_with("HEVC") {
                VideoCodec::Hevc
            } else {
                VideoCodec::Other
            };
        }
        if let Some(rest) = line.strip_prefix("a=control:") {
            control = rest.trim().to_string();
        }
    }
    if codec == VideoCodec::Other && sdp.to_ascii_uppercase().contains("H264") {
        codec = VideoCodec::H264;
    }
    if !sdp.contains("m=video") && codec == VideoCodec::Other {
        return None;
    }
    if control.is_empty() {
        control = format!("{content_base}/trackID=0");
    } else {
        control = join_control(&content_base, &control);
    }
    Some(SdpVideo {
        codec,
        payload,
        control,
    })
}

fn join_control(base: &str, control: &str) -> String {
    if control.eq("*") {
        return base.to_string();
    }
    if control.starts_with("rtsp://") || control.starts_with("rtsps://") {
        return control.replacen("rtsps://", "rtsp://", 1);
    }
    if base.ends_with('/') {
        format!("{base}{control}")
    } else {
        format!("{base}/{control}")
    }
}

pub(crate) fn rtp_payload(packet: &[u8]) -> Option<&[u8]> {
    if packet.len() < 12 {
        return None;
    }
    if packet[0] >> 6 != 2 {
        return None;
    }
    let cc = (packet[0] & 0x0f) as usize;
    let extension = packet[0] & 0x10 != 0;
    let padding = packet[0] & 0x20 != 0;
    let mut offset = 12 + cc * 4;
    if packet.len() < offset {
        return None;
    }
    if extension {
        if packet.len() < offset + 4 {
            return None;
        }
        let ext_len = u16::from_be_bytes([packet[offset + 2], packet[offset + 3]]) as usize;
        offset += 4 + ext_len * 4;
    }
    if packet.len() < offset {
        return None;
    }
    let mut end = packet.len();
    if padding {
        let pad = packet[end - 1] as usize;
        end = end.saturating_sub(pad);
    }
    if offset >= end {
        None
    } else {
        Some(&packet[offset..end])
    }
}

#[derive(Default)]
pub(crate) struct H264Depay {
    fu: Vec<u8>,
}

impl H264Depay {
    pub(crate) fn push(&mut self, payload: &[u8]) -> Vec<Vec<u8>> {
        if payload.is_empty() {
            return Vec::new();
        }
        let nal_type = payload[0] & 0x1f;
        match nal_type {
            1..=23 => vec![annexb(payload)],
            24 => stap_a(payload),
            28 => self.fu_a(payload).into_iter().collect(),
            _ => Vec::new(),
        }
    }

    fn fu_a(&mut self, payload: &[u8]) -> Option<Vec<u8>> {
        if payload.len() < 3 {
            return None;
        }
        let fu_indicator = payload[0];
        let fu_header = payload[1];
        let start = fu_header & 0x80 != 0;
        let end = fu_header & 0x40 != 0;
        let nal_type = fu_header & 0x1f;
        let nal_header = (fu_indicator & 0xe0) | nal_type;
        if start {
            self.fu.clear();
            self.fu.push(nal_header);
        } else if self.fu.is_empty() {
            return None;
        }
        self.fu.extend_from_slice(&payload[2..]);
        if end {
            let nal = std::mem::take(&mut self.fu);
            Some(annexb(&nal))
        } else {
            None
        }
    }
}

fn stap_a(payload: &[u8]) -> Vec<Vec<u8>> {
    let mut out = Vec::new();
    let mut i = 1;
    while i + 2 <= payload.len() {
        let len = u16::from_be_bytes([payload[i], payload[i + 1]]) as usize;
        i += 2;
        if i + len > payload.len() {
            break;
        }
        out.push(annexb(&payload[i..i + len]));
        i += len;
    }
    out
}

fn annexb(nal: &[u8]) -> Vec<u8> {
    let mut out = Vec::with_capacity(4 + nal.len());
    out.extend_from_slice(&[0, 0, 0, 1]);
    out.extend_from_slice(nal);
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn sdp_h264_control_is_joined() {
        let sdp = "\
v=0\r\n\
m=video 0 RTP/AVP 96\r\n\
a=rtpmap:96 H264/90000\r\n\
a=control:trackID=0\r\n";
        let video = parse_sdp_video(sdp, "rtsp://192.168.1.9:322/streaming/live/1").unwrap();
        assert_eq!(video.codec, VideoCodec::H264);
        assert_eq!(video.payload, 96);
        assert_eq!(
            video.control,
            "rtsp://192.168.1.9:322/streaming/live/1/trackID=0"
        );
    }

    #[test]
    fn sdp_hevc_is_detected() {
        let sdp = "m=video 0 RTP/AVP 96\na=rtpmap:96 H265/90000\na=control:trackID=1\n";
        let video = parse_sdp_video(sdp, "rtsp://h:322/streaming/live/1").unwrap();
        assert_eq!(video.codec, VideoCodec::Hevc);
    }

    #[test]
    fn rtp_skips_header() {
        let mut pkt = vec![0x80, 96, 0, 1, 0, 0, 0, 0, 0, 0, 0, 0];
        pkt.extend_from_slice(&[0x67, 0x42]);
        assert_eq!(rtp_payload(&pkt), Some(&[0x67, 0x42][..]));
    }

    #[test]
    fn fu_a_reassembles_nal() {
        let mut depay = H264Depay::default();
        let start = [0x7c, 0x85, 0xaa];
        let end = [0x7c, 0x45, 0xbb];
        assert!(depay.push(&start).is_empty());
        let nals = depay.push(&end);
        assert_eq!(nals.len(), 1);
        assert_eq!(&nals[0][..4], &[0, 0, 0, 1]);
        assert_eq!(nals[0][4], 0x65);
        assert_eq!(&nals[0][5..], &[0xaa, 0xbb]);
    }

    #[test]
    fn stap_a_splits_nals() {
        let payload = [0x18, 0x00, 0x02, 0x67, 0x42, 0x00, 0x02, 0x68, 0xce];
        let nals = stap_a(&payload);
        assert_eq!(nals.len(), 2);
        assert_eq!(nals[0], vec![0, 0, 0, 1, 0x67, 0x42]);
        assert_eq!(nals[1], vec![0, 0, 0, 1, 0x68, 0xce]);
    }

    #[test]
    fn digest_response_matches_rfc2617_example() {
        let challenge = DigestChallenge {
            realm: "testrealm@host.com".into(),
            nonce: "dcd98b7102dd2f0e8b11d0f600bfb0c093".into(),
        };
        let header = digest_auth(
            "Mufasa",
            "Circle Of Life",
            &challenge,
            "GET",
            "/dir/index.html",
        );
        let ha1 = md5_hex(b"Mufasa:testrealm@host.com:Circle Of Life");
        let ha2 = md5_hex(b"GET:/dir/index.html");
        let response =
            md5_hex(format!("{ha1}:dcd98b7102dd2f0e8b11d0f600bfb0c093:{ha2}").as_bytes());
        assert!(header.contains(&format!("response=\"{response}\"")));
    }

    #[test]
    fn session_strips_timeout() {
        assert_eq!(parse_session("1A2B;timeout=60").as_deref(), Some("1A2B"));
    }
}
