//! VocsClient / ChooseServer without `libagora_rtc_sdk`.
//!
//! Wire types come from Agora's MIT `@agora-js/protocol` `signal.proto`
//! (SdpEndpointRequest uri=22, service 11). HTTP AP hosts are the published
//! WEBCS domains. Encoded H.264 on the edge is depayed like LAN RTSPS.

use std::io::{Read, Write};
use std::net::UdpSocket;
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

use flate2::read::DeflateDecoder;
use flate2::write::DeflateEncoder;
use flate2::Compression;
use openh264::decoder::Decoder;
use rand::Rng;

use bambu_device::Frame;

use crate::agora::{
    encryption_config, on_encoded_video_frame, AgoraJoin, RtcSession, AREA_CODE_AS, AREA_CODE_CN,
    AREA_CODE_EU, AREA_CODE_NA, CLIENT_ROLE_AUDIENCE,
};
use crate::camera::CameraError;
use crate::https;
use crate::rtsps::{rtp_payload, H264Depay};

const SERVICE_CHOOSE_SERVER: u32 = 11;
const UNILBS_URI: u32 = 22;
const WEB_SDK_VERSION: &str = "4.2.0";

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct VosEdge {
    pub ip: String,
    pub port: u16,
    pub cid: u32,
    pub uid: u32,
    pub ticket: String,
}

pub fn webcs_hosts(area: u32) -> &'static [&'static str] {
    match area {
        AREA_CODE_CN => &[
            "webrtc2-ap-web-1.agora.io",
            "webrtc2-2.ap.sd-rtn.com",
            "ap-web-1.agora.io",
        ],
        AREA_CODE_AS => &["ap-web-1-asia.agora.io", "ap-web-2-asia.agora.io"],
        AREA_CODE_EU => &["ap-web-1-europe.agora.io", "ap-web-2-europe.agora.io"],
        AREA_CODE_NA => &[
            "ap-web-1-north-america.agora.io",
            "webrtc2-ap-web-1.agora.io",
            "webrtc2-2.ap.sd-rtn.com",
        ],
        _ => &[
            "webrtc2-ap-web-1.agora.io",
            "webrtc2-2.ap.sd-rtn.com",
            "webrtc2-ap-web-3.agora.io",
        ],
    }
}

pub fn web_area_name(area: u32) -> &'static str {
    match area {
        AREA_CODE_CN => "CHINA",
        AREA_CODE_AS => "ASIA",
        AREA_CODE_EU => "EUROPE",
        AREA_CODE_NA => "NORTH_AMERICA",
        _ => "GLOBAL",
    }
}

pub fn is_agora_token(token: &str) -> bool {
    let t = token.trim();
    t.starts_with("006") || t.starts_with("007")
}

pub fn encode_sdp_endpoint_request(join: &AgoraJoin, session: &RtcSession) -> Vec<u8> {
    let mut rng = rand::thread_rng();
    let opid: u64 = rng.gen_range(1..1_000_000_000_000);
    let session_id: String = (0..16)
        .map(|_| format!("{:x}", rng.gen_range(0..16u8)))
        .collect();
    let area = web_area_name(session.engine.area_code);
    let role = if session.options.client_role == CLIENT_ROLE_AUDIENCE {
        "audience"
    } else {
        "host"
    };
    let detail_role = if role == "audience" { "2" } else { "1" };

    let mut signal = Vec::new();
    put_u32(&mut signal, 2, 0); // p2p_id
    put_str(&mut signal, 3, &session_id);
    put_str(&mut signal, 4, &join.app_id);
    put_str(&mut signal, 5, &join.token);
    put_str(&mut signal, 6, &join.channel);
    put_str(&mut signal, 7, WEB_SDK_VERSION);
    put_str(&mut signal, 10, "live");
    put_str(&mut signal, 11, "h264");
    put_str(&mut signal, 12, role);
    put_u64(&mut signal, 19, unix_ms());
    if let Some(enc) = encryption_config(join) {
        put_str(&mut signal, 21, "aes-128-gcm2");
        put_str(&mut signal, 22, &enc.key);
        put_bool(&mut signal, 23, true);
        put_str(
            &mut signal,
            24,
            &base64::Engine::encode(&base64::engine::general_purpose::STANDARD, &enc.salt),
        );
    }

    let struct_bytes =
        encode_struct(&[("11", area), ("22", area), ("12", "1"), ("17", detail_role)]);
    let mut unilbs = Vec::new();
    put_u32(&mut unilbs, 1, UNILBS_URI);
    put_msg(&mut unilbs, 2, &struct_bytes);
    let mut packed = Vec::new();
    put_varint(&mut packed, u64::from(SERVICE_CHOOSE_SERVER));
    put_bytes(&mut unilbs, 3, &packed);

    let mut req = Vec::new();
    put_msg(&mut req, 1, &unilbs);
    put_u32(&mut req, 2, session.connection.local_uid);
    put_u64(&mut req, 3, opid);
    put_msg(&mut req, 4, &signal);
    req
}

pub fn deflate_raw(bytes: &[u8]) -> Result<Vec<u8>, CameraError> {
    let mut enc = DeflateEncoder::new(Vec::new(), Compression::default());
    enc.write_all(bytes)
        .map_err(|err| CameraError::Message(err.to_string()))?;
    enc.finish()
        .map_err(|err| CameraError::Message(err.to_string()))
}

pub fn inflate_raw(bytes: &[u8]) -> Result<Vec<u8>, CameraError> {
    let mut dec = DeflateDecoder::new(bytes);
    let mut out = Vec::new();
    dec.read_to_end(&mut out)
        .map_err(|err| CameraError::Message(err.to_string()))?;
    Ok(out)
}

pub fn parse_sdp_endpoint_response(buf: &[u8]) -> Result<Vec<VosEdge>, CameraError> {
    if let Some(edges) = parse_sdp_json(buf) {
        return Ok(edges);
    }
    if let Ok(raw) = inflate_raw(buf) {
        if let Some(edges) = parse_sdp_json(&raw) {
            return Ok(edges);
        }
        if let Ok(edges) = parse_sdp_protobuf(&raw) {
            if !edges.is_empty() {
                return Ok(edges);
            }
        }
    }
    parse_sdp_protobuf(buf)
}

fn parse_sdp_json(raw: &[u8]) -> Option<Vec<VosEdge>> {
    let text = std::str::from_utf8(raw).ok()?;
    let v: serde_json::Value = serde_json::from_str(text).ok()?;
    let uid = v.get("uid").and_then(|x| x.as_u64()).unwrap_or(0) as u32;
    let list = v
        .get("unilbs_response")
        .or_else(|| v.get("unilbsResponse"))
        .or_else(|| v.get("response_body"))?;
    let mut edges = Vec::new();
    for item in list.as_array()? {
        let body = item.get("buffer").unwrap_or(item);
        let cid = body.get("cid").and_then(|x| x.as_u64()).unwrap_or(0) as u32;
        let ticket = body
            .get("cert")
            .or_else(|| body.get("ticket"))
            .and_then(|x| x.as_str())
            .unwrap_or("")
            .to_string();
        let services = body
            .get("services")
            .or_else(|| body.get("edges_services"))
            .and_then(|x| x.as_array())?;
        for svc in services {
            let ip = svc.get("ip").and_then(|x| x.as_str()).unwrap_or("");
            let port = svc.get("port").and_then(|x| x.as_u64()).unwrap_or(0) as u16;
            if ip.is_empty() || port == 0 {
                continue;
            }
            edges.push(VosEdge {
                ip: ip.to_string(),
                port,
                cid,
                uid,
                ticket: ticket.clone(),
            });
        }
    }
    Some(edges)
}

fn parse_sdp_protobuf(raw: &[u8]) -> Result<Vec<VosEdge>, CameraError> {
    let fields = parse_fields(raw)?;
    let uid = fields
        .iter()
        .find(|(n, _)| *n == 6)
        .and_then(|(_, v)| v.as_varint())
        .unwrap_or(0) as u32;
    let mut edges = Vec::new();
    for (_, val) in fields.iter().filter(|(n, _)| *n == 5) {
        let Some(msg) = val.as_bytes() else {
            continue;
        };
        let inner = parse_fields(msg)?;
        let cert = inner
            .iter()
            .find(|(n, _)| *n == 1)
            .and_then(|(_, v)| v.as_str())
            .unwrap_or("")
            .to_string();
        let cid = inner
            .iter()
            .find(|(n, _)| *n == 2)
            .and_then(|(_, v)| v.as_varint())
            .unwrap_or(0) as u32;
        for (_, svc) in inner.iter().filter(|(n, _)| *n == 5) {
            let Some(bytes) = svc.as_bytes() else {
                continue;
            };
            let svc_fields = parse_fields(bytes)?;
            let ip = svc_fields
                .iter()
                .find(|(n, _)| *n == 1)
                .and_then(|(_, v)| v.as_str())
                .unwrap_or("")
                .to_string();
            let port = svc_fields
                .iter()
                .find(|(n, _)| *n == 2)
                .and_then(|(_, v)| v.as_varint())
                .unwrap_or(0) as u16;
            if ip.is_empty() || port == 0 {
                continue;
            }
            edges.push(VosEdge {
                ip,
                port,
                cid,
                uid,
                ticket: cert.clone(),
            });
        }
    }
    Ok(edges)
}

pub fn choose_server(join: &AgoraJoin, session: &RtcSession) -> Result<Vec<VosEdge>, CameraError> {
    let proto = encode_sdp_endpoint_request(join, session);
    let deflated = deflate_raw(&proto)?;
    let mut last = String::from("no AP host");
    for host in webcs_hosts(session.engine.area_code) {
        match post_ap(host, &deflated).or_else(|_| post_ap(host, &proto)) {
            Ok(body) => {
                let edges = parse_sdp_endpoint_response(&body)?;
                if !edges.is_empty() {
                    return Ok(edges);
                }
                last = format!("{host} returned no VOS edges");
            }
            Err(err) => last = err.to_string(),
        }
    }
    Err(CameraError::Message(format!(
        "agora rtc: joinChannelEx AP choose-server failed ({last})"
    )))
}

fn post_ap(host: &str, body: &[u8]) -> Result<Vec<u8>, CameraError> {
    let headers = [
        ("Content-Type", "text/plain"),
        ("Accept", "*/*"),
        ("X-Packet-URI", "22"),
        ("X-Packet-Service-Type", "11"),
    ];
    let resp = https::request("POST", host, "/api/v1", &headers, Some(body))
        .map_err(|err| CameraError::Message(err.to_string()))?;
    if resp.status < 200 || resp.status >= 300 {
        return Err(CameraError::Message(format!(
            "agora AP HTTP {} on {host}",
            resp.status
        )));
    }
    Ok(resp.body)
}

/// Native VOS login fields from `Login VOS (proto, cid, uid, role, ts, ticket)`.
pub fn encode_vos_login(join: &AgoraJoin, session: &RtcSession, edge: &VosEdge) -> Vec<u8> {
    let mut msg = Vec::new();
    put_u32(&mut msg, 1, 2); // proto / VOS2
    put_u32(&mut msg, 2, edge.cid);
    put_u32(
        &mut msg,
        3,
        if edge.uid == 0 {
            session.connection.local_uid
        } else {
            edge.uid
        },
    );
    put_u32(&mut msg, 4, session.options.client_role);
    put_u64(&mut msg, 5, unix_ms());
    if !edge.ticket.is_empty() {
        put_bytes(&mut msg, 6, edge.ticket.as_bytes());
    }
    put_str(&mut msg, 7, &join.channel);
    put_str(&mut msg, 8, &join.app_id);
    put_str(&mut msg, 9, &join.token);
    if session.encryption.is_some() {
        put_u32(&mut msg, 10, 7); // AES_128_GCM2
    }
    let mut framed = Vec::with_capacity(4 + msg.len());
    framed.extend_from_slice(&(msg.len() as u16).to_be_bytes());
    framed.extend_from_slice(&(SERVICE_CHOOSE_SERVER as u16).to_be_bytes());
    framed.extend_from_slice(&msg);
    framed
}

pub fn stream_vos_frames(
    join: &AgoraJoin,
    session: &RtcSession,
    mut on_frame: impl FnMut(Frame) -> bool,
) -> Result<(), CameraError> {
    let edges = choose_server(join, session)?;
    let mut decoder = Decoder::new().map_err(|err| CameraError::Message(err.to_string()))?;
    let mut depay = H264Depay::default();
    let mut last = String::from("no VOS edge");
    for edge in &edges {
        match subscribe_edge(edge, join, session, &mut decoder, &mut depay, &mut on_frame) {
            Ok(()) => return Ok(()),
            Err(err) => last = err.to_string(),
        }
    }
    Err(CameraError::Message(format!(
        "agora rtc: joinChannelEx VOS subscribe failed ({last})"
    )))
}

fn subscribe_edge(
    edge: &VosEdge,
    join: &AgoraJoin,
    session: &RtcSession,
    decoder: &mut Decoder,
    depay: &mut H264Depay,
    on_frame: &mut impl FnMut(Frame) -> bool,
) -> Result<(), CameraError> {
    let sock = UdpSocket::bind("0.0.0.0:0")?;
    sock.set_read_timeout(Some(Duration::from_millis(800)))?;
    sock.set_write_timeout(Some(Duration::from_secs(2)))?;
    let addr = format!("{}:{}", edge.ip, edge.port);
    sock.connect(&addr)?;
    let login = encode_vos_login(join, session, edge);
    sock.send(&login)?;
    sock.send(&login[4..])?;
    let deadline = Instant::now() + Duration::from_secs(8);
    let mut buf = [0u8; 2048];
    let mut saw = false;
    while Instant::now() < deadline {
        let n = match sock.recv(&mut buf) {
            Ok(n) => n,
            Err(err)
                if err.kind() == std::io::ErrorKind::WouldBlock
                    || err.kind() == std::io::ErrorKind::TimedOut =>
            {
                continue;
            }
            Err(err) => return Err(err.into()),
        };
        saw = true;
        for nal in annexb_or_rtp(depay, &buf[..n]) {
            match on_encoded_video_frame(decoder, &nal) {
                Ok(Some(frame)) => {
                    if !on_frame(frame) {
                        return Ok(());
                    }
                }
                Ok(None) | Err(_) => {}
            }
        }
    }
    if saw {
        Ok(())
    } else {
        Err(CameraError::Message(format!(
            "VOS {addr} produced no datagrams"
        )))
    }
}

fn annexb_or_rtp(depay: &mut H264Depay, packet: &[u8]) -> Vec<Vec<u8>> {
    if let Some(start) = packet.windows(4).position(|w| w == [0, 0, 0, 1]) {
        return vec![packet[start..].to_vec()];
    }
    if let Some(payload) = rtp_payload(packet) {
        return depay.push(payload);
    }
    Vec::new()
}

fn unix_ms() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_millis() as u64
}

fn put_varint(buf: &mut Vec<u8>, mut v: u64) {
    while v >= 0x80 {
        buf.push((v as u8) | 0x80);
        v >>= 7;
    }
    buf.push(v as u8);
}

fn put_tag(buf: &mut Vec<u8>, field: u32, wire: u32) {
    put_varint(buf, u64::from((field << 3) | wire));
}

fn put_u32(buf: &mut Vec<u8>, field: u32, v: u32) {
    put_tag(buf, field, 0);
    put_varint(buf, u64::from(v));
}

fn put_u64(buf: &mut Vec<u8>, field: u32, v: u64) {
    put_tag(buf, field, 0);
    put_varint(buf, v);
}

fn put_bool(buf: &mut Vec<u8>, field: u32, v: bool) {
    put_u32(buf, field, u32::from(v));
}

fn put_bytes(buf: &mut Vec<u8>, field: u32, data: &[u8]) {
    put_tag(buf, field, 2);
    put_varint(buf, data.len() as u64);
    buf.extend_from_slice(data);
}

fn put_str(buf: &mut Vec<u8>, field: u32, s: &str) {
    put_bytes(buf, field, s.as_bytes());
}

fn put_msg(buf: &mut Vec<u8>, field: u32, msg: &[u8]) {
    put_bytes(buf, field, msg);
}

fn encode_struct(pairs: &[(&str, &str)]) -> Vec<u8> {
    let mut out = Vec::new();
    for (k, v) in pairs {
        let mut value = Vec::new();
        put_str(&mut value, 3, v);
        let mut entry = Vec::new();
        put_str(&mut entry, 1, k);
        put_msg(&mut entry, 2, &value);
        put_msg(&mut out, 1, &entry);
    }
    out
}

#[derive(Debug)]
enum Wire {
    Varint(u64),
    Bytes(Vec<u8>),
}

impl Wire {
    fn as_varint(&self) -> Option<u64> {
        match self {
            Wire::Varint(v) => Some(*v),
            Wire::Bytes(_) => None,
        }
    }

    fn as_bytes(&self) -> Option<&[u8]> {
        match self {
            Wire::Bytes(b) => Some(b),
            Wire::Varint(_) => None,
        }
    }

    fn as_str(&self) -> Option<&str> {
        self.as_bytes().and_then(|b| std::str::from_utf8(b).ok())
    }
}

fn parse_fields(buf: &[u8]) -> Result<Vec<(u32, Wire)>, CameraError> {
    let mut i = 0;
    let mut out = Vec::new();
    while i < buf.len() {
        let (tag, n) = read_varint(&buf[i..])?;
        i += n;
        let field = (tag >> 3) as u32;
        let wire = (tag & 7) as u32;
        match wire {
            0 => {
                let (v, n) = read_varint(&buf[i..])?;
                i += n;
                out.push((field, Wire::Varint(v)));
            }
            2 => {
                let (len, n) = read_varint(&buf[i..])?;
                i += n;
                let len = len as usize;
                if i + len > buf.len() {
                    return Err(CameraError::Message("agora protobuf truncated".into()));
                }
                out.push((field, Wire::Bytes(buf[i..i + len].to_vec())));
                i += len;
            }
            1 => {
                if i + 8 > buf.len() {
                    return Err(CameraError::Message("agora protobuf truncated".into()));
                }
                i += 8;
            }
            5 => {
                if i + 4 > buf.len() {
                    return Err(CameraError::Message("agora protobuf truncated".into()));
                }
                i += 4;
            }
            _ => {
                return Err(CameraError::Message(format!("agora protobuf wire {wire}")));
            }
        }
    }
    Ok(out)
}

fn read_varint(buf: &[u8]) -> Result<(u64, usize), CameraError> {
    let mut v = 0u64;
    let mut shift = 0;
    for (i, b) in buf.iter().enumerate() {
        v |= u64::from(b & 0x7f) << shift;
        if b & 0x80 == 0 {
            return Ok((v, i + 1));
        }
        shift += 7;
        if shift > 63 {
            break;
        }
    }
    Err(CameraError::Message("agora protobuf varint".into()))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::agora::AgoraJoin;
    use crate::cloud_api::CameraCreds;

    fn sample_join() -> (AgoraJoin, RtcSession) {
        let creds = CameraCreds {
            channel: "devchan".into(),
            token: format!("006{}rest", "ab".repeat(16)),
            app_id: "appidappidappidappidappidappidap".into(),
            region: "us".into(),
            user: "42".into(),
            stream_key: "k".into(),
            stream_salt: "AQID".into(),
            ..CameraCreds::default()
        };
        let join = AgoraJoin::from_creds(&creds).unwrap();
        let session = RtcSession::prepare(&join).unwrap();
        (join, session)
    }

    #[test]
    fn sdp_request_carries_choose_server_and_channel() {
        let (join, session) = sample_join();
        let bytes = encode_sdp_endpoint_request(&join, &session);
        assert!(bytes
            .windows(join.channel.len())
            .any(|w| w == join.channel.as_bytes()));
        assert!(bytes
            .windows(join.app_id.len())
            .any(|w| w == join.app_id.as_bytes()));
        let fields = parse_fields(&bytes).unwrap();
        assert!(fields.iter().any(|(n, _)| *n == 1));
        assert!(fields.iter().any(|(n, _)| *n == 4));
        let unilbs = fields
            .iter()
            .find(|(n, _)| *n == 1)
            .unwrap()
            .1
            .as_bytes()
            .unwrap();
        let inner = parse_fields(unilbs).unwrap();
        assert_eq!(
            inner.iter().find(|(n, _)| *n == 1).unwrap().1.as_varint(),
            Some(u64::from(UNILBS_URI))
        );
        let packed = inner
            .iter()
            .find(|(n, _)| *n == 3)
            .unwrap()
            .1
            .as_bytes()
            .unwrap();
        assert_eq!(packed, &[SERVICE_CHOOSE_SERVER as u8]);
    }

    #[test]
    fn deflate_roundtrip() {
        let raw = b"agora-choose-server";
        let z = deflate_raw(raw).unwrap();
        assert_ne!(z, raw);
        assert_eq!(inflate_raw(&z).unwrap(), raw);
    }

    #[test]
    fn parse_json_edges() {
        let json = br#"{
            "uid": 9,
            "unilbsResponse": [{
                "cid": 44,
                "cert": "tick",
                "services": [{"ip": "1.2.3.4", "port": 4001}]
            }]
        }"#;
        let edges = parse_sdp_endpoint_response(json).unwrap();
        assert_eq!(edges.len(), 1);
        assert_eq!(edges[0].ip, "1.2.3.4");
        assert_eq!(edges[0].port, 4001);
        assert_eq!(edges[0].cid, 44);
        assert_eq!(edges[0].ticket, "tick");
        assert_eq!(edges[0].uid, 9);
    }

    #[test]
    fn parse_protobuf_edges() {
        let mut svc = Vec::new();
        put_str(&mut svc, 1, "8.8.8.8");
        put_u32(&mut svc, 2, 8443);
        let mut unilbs = Vec::new();
        put_str(&mut unilbs, 1, "cert1");
        put_u32(&mut unilbs, 2, 7);
        put_msg(&mut unilbs, 5, &svc);
        let mut resp = Vec::new();
        put_msg(&mut resp, 5, &unilbs);
        put_u32(&mut resp, 6, 99);
        let edges = parse_sdp_endpoint_response(&resp).unwrap();
        assert_eq!(edges[0].ip, "8.8.8.8");
        assert_eq!(edges[0].port, 8443);
        assert_eq!(edges[0].cid, 7);
        assert_eq!(edges[0].uid, 99);
        assert_eq!(edges[0].ticket, "cert1");
    }

    #[test]
    fn vos_login_frames_ticket() {
        let (join, session) = sample_join();
        let edge = VosEdge {
            ip: "127.0.0.1".into(),
            port: 4001,
            cid: 3,
            uid: 42,
            ticket: "TICKET".into(),
        };
        let pkt = encode_vos_login(&join, &session, &edge);
        assert!(pkt.windows(6).any(|w| w == b"TICKET"));
        assert!(u16::from_be_bytes([pkt[0], pkt[1]]) as usize + 4 == pkt.len());
    }

    #[test]
    fn token_detects_006() {
        assert!(is_agora_token(&format!("006{}", "a".repeat(32))));
        assert!(!is_agora_token("tok"));
    }
}
