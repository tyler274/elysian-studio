//! Cloud liveview after `POST /v1/iot-service/api/user/ttcode`.
//!
//! TUTK JPEG and Agora minting live here. Agora frames go through `agora` /
//! `agora_ap` (no `libBambuSource` / `libbambu_networking`).

use std::net::{Ipv4Addr, SocketAddr, ToSocketAddrs, UdpSocket};
use std::time::{Duration, Instant};

use bambu_device::Frame;

use crate::camera::{jpeg_to_frame, CameraError, JpegStream};
use crate::cloud_api::{CameraCreds, CameraProto, CloudApi};
use crate::oauth::percent_encode_query;

/// TUTK `MEDIA_CODEC_VIDEO_H264` (`0x4E`).
pub const CODEC_H264: u16 = 0x4E;
/// TUTK `MEDIA_CODEC_VIDEO_MJPEG` (`0x4F`).
pub const CODEC_MJPEG: u16 = 0x4F;
/// TUTK `MEDIA_CODEC_VIDEO_HEVC` (`0x50`).
pub const CODEC_HEVC: u16 = 0x50;

/// Public Kalay IOTC masters (same names as `IOTC_Initialize`).
pub const IOTC_MASTERS_GLOBAL: &[&str] = &[
    "m1.iotcplatform.com",
    "m2.iotcplatform.com",
    "m4.iotcplatform.com",
    "m5.iotcplatform.com",
    "us.iotcplatform.com",
];

pub const IOTC_MASTERS_CN: &[&str] = &[
    "m1.kalay.net.cn",
    "m2.kalay.net.cn",
    "cn.iotcplatform.com",
    "m1.iotcplatform.com",
];

/// UDP ports TUTK clients probe on IOTC masters (device call-home is 10000–19999).
const IOTC_QUERY_PORTS: &[u16] = &[80, 443, 8000, 10000, 10001, 8080];

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TutkRegion {
    Us,
    Cn,
    Eu,
    Asia,
}

/// Studio / `TUTK_SDK_Set_Region` mapping from ttcode `region=`.
pub fn tutk_region(region: &str) -> TutkRegion {
    match region.trim().to_ascii_lowercase().as_str() {
        "cn" | "china" | "zh" => TutkRegion::Cn,
        "eu" | "europe" => TutkRegion::Eu,
        "asia" | "as" | "jp" | "kr" => TutkRegion::Asia,
        _ => TutkRegion::Us,
    }
}

pub fn iotc_masters(region: &str) -> &'static [&'static str] {
    match tutk_region(region) {
        TutkRegion::Cn => IOTC_MASTERS_CN,
        _ => IOTC_MASTERS_GLOBAL,
    }
}

/// TUTK `FRAMEINFO_t` prefix on `avRecvFrameData2`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct AvFrameInfo {
    pub codec_id: u16,
    pub flags: u8,
    pub cam_index: u8,
}

impl AvFrameInfo {
    pub fn parse(bytes: &[u8]) -> Option<Self> {
        if bytes.len() < 4 {
            return None;
        }
        Some(Self {
            codec_id: u16::from_le_bytes([bytes[0], bytes[1]]),
            flags: bytes[2],
            cam_index: bytes[3],
        })
    }

    pub fn is_mjpeg(self) -> bool {
        self.codec_id == CODEC_MJPEG
    }

    pub fn is_avc(self) -> bool {
        self.codec_id == CODEC_H264 || self.codec_id == CODEC_HEVC
    }
}

/// JPEG payload from a TUTK AV sample (P1/A1 MJPEG). H.264/HEVC is skipped.
pub fn jpeg_from_av_sample(payload: &[u8]) -> Option<&[u8]> {
    if payload.len() >= 2 && payload[0] == 0xFF && payload[1] == 0xD8 {
        return Some(payload);
    }
    if let Some(info) = AvFrameInfo::parse(payload) {
        if info.is_mjpeg() {
            let rest = payload.get(16..)?;
            if rest.len() >= 2 && rest[0] == 0xFF && rest[1] == 0xD8 {
                return Some(rest);
            }
        }
    }
    None
}

pub fn is_avc_sample(payload: &[u8]) -> bool {
    if jpeg_from_av_sample(payload).is_some() {
        return false;
    }
    AvFrameInfo::parse(payload).is_some_and(AvFrameInfo::is_avc)
        || payload
            .first()
            .is_some_and(|&b| b == 0x00 || b == 0x65 || b == 0x67)
}

fn ttcode_error(err: &impl std::fmt::Display) -> String {
    let text = err.to_string();
    if crate::cloud_api::cloud_error_is_rate_limited(&text) {
        format!(
            "ttcode: {text} Studio/Handy mint the same endpoint; Cloudflare 1015 is IP rate-limit, not a printer setting."
        )
    } else if text.contains("HTTP 403") || text.to_ascii_lowercase().contains("forbidden") {
        format!(
            "ttcode: {text}. Cloud Agora mint is POST /ttcode (Handy works off Wi‑Fi). 403 is the token, serial, or user-id header, not a printer LAN toggle."
        )
    } else {
        format!("ttcode: {text}")
    }
}

/// `POST .../ttcode` then TUTK MJPEG (or Agora error). Token refresh on 401.
/// When IOTC reports a reachable IPv4 and `lan_code` is set, frames come from
/// the existing JPEG :6000 client (same as LAN Play).
pub fn stream_ttcode_jpegs(
    api: &mut CloudApi,
    serial: &str,
    lan_code: &str,
    on_jpeg: impl FnMut(&[u8]) -> bool,
) -> Result<(), CameraError> {
    if serial.is_empty() {
        return Err(CameraError::Message(
            "cloud camera needs a device serial".into(),
        ));
    }
    let creds = api
        .with_retry(|api| api.mint_camera_creds(serial))
        .map_err(|err| CameraError::Message(ttcode_error(&err)))?;
    for_each_jpeg(&creds, lan_code, on_jpeg)
}

/// Mint `get_camera_url` then yield RGBA frames (Agora encoded observer or TUTK JPEG).
pub fn stream_ttcode_frames(
    api: &mut CloudApi,
    serial: &str,
    lan_code: &str,
    firmware: Option<&str>,
    mut on_frame: impl FnMut(Frame) -> bool,
) -> Result<(), CameraError> {
    if serial.is_empty() {
        return Err(CameraError::Message(
            "cloud camera needs a device serial".into(),
        ));
    }
    let mut creds = api
        .with_retry(|api| api.mint_camera_creds_with(serial, firmware))
        .map_err(|err| CameraError::Message(ttcode_error(&err)))?;
    if creds.device.is_empty() {
        creds.device = serial.to_string();
    }
    tracing::debug!(
        target: "bambu_protocol::cloud",
        proto = ?creds.proto,
        device = %crate::cloud_api::redact_id(&creds.device),
        has_channel = !creds.channel.is_empty(),
        has_token = !creds.token.is_empty(),
        "minted camera session"
    );
    match creds.proto {
        CameraProto::Agora => crate::agora::stream_agora_frames(&creds, on_frame),
        CameraProto::Tutk => {
            tutk_recv_jpegs(&creds, lan_code, &mut |jpeg| match jpeg_to_frame(jpeg) {
                Ok(frame) => on_frame(frame),
                Err(_) => true,
            })
        }
    }
}

/// Open a `bambu:///tutk` or `bambu:///agora` session and yield JPEG payloads.
pub fn for_each_jpeg(
    creds: &CameraCreds,
    lan_code: &str,
    mut on_jpeg: impl FnMut(&[u8]) -> bool,
) -> Result<(), CameraError> {
    match creds.proto {
        CameraProto::Agora => crate::agora::stream_agora_frames(creds, |_| true),
        CameraProto::Tutk => tutk_recv_jpegs(creds, lan_code, &mut on_jpeg),
    }
}

fn tutk_recv_jpegs(
    creds: &CameraCreds,
    lan_code: &str,
    on_jpeg: &mut dyn FnMut(&[u8]) -> bool,
) -> Result<(), CameraError> {
    if creds.uid.is_empty() {
        return Err(CameraError::Message("TUTK uid/ttcode is empty".into()));
    }
    if creds.authkey.is_empty() || creds.passwd.is_empty() {
        return Err(CameraError::Message(
            "TUTK authkey/passwd missing from ttcode".into(),
        ));
    }
    let peer = iotc_locate(&creds.uid, &creds.authkey, &creds.region)?;
    if !lan_code.is_empty() {
        match JpegStream::connect(&peer.ip.to_string(), lan_code) {
            Ok(mut stream) => loop {
                let jpeg = stream.next_jpeg()?;
                if !on_jpeg(&jpeg) {
                    return Ok(());
                }
            },
            Err(_) => {}
        }
    }
    Err(CameraError::Message(format!(
        "TUTK IOTC peer {} (uid {}, region {}). JPEG :6000 needs a LAN access code; AV recv (avClientStartEx) is not on this datagram yet",
        peer, creds.uid, creds.region
    )))
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct IotcPeer {
    pub ip: Ipv4Addr,
    pub port: u16,
}

impl std::fmt::Display for IotcPeer {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{}:{}", self.ip, self.port)
    }
}

/// Query Kalay masters for the device behind `uid` (IOTC_Connect_ByUIDEx lookup).
pub fn iotc_locate(uid: &str, authkey: &str, region: &str) -> Result<IotcPeer, CameraError> {
    let uid = uid.trim();
    if uid.is_empty() {
        return Err(CameraError::Message("empty TUTK uid".into()));
    }
    let query = iotc_uid_query(uid, authkey);
    let socket = UdpSocket::bind("0.0.0.0:0")?;
    socket.set_read_timeout(Some(Duration::from_millis(400)))?;
    socket.set_write_timeout(Some(Duration::from_secs(2)))?;
    let mut sent = 0usize;
    let mut last_dns = String::new();
    for host in iotc_masters(region) {
        let addrs = match (*host, 80u16).to_socket_addrs() {
            Ok(addrs) => addrs.collect::<Vec<_>>(),
            Err(err) => {
                last_dns = format!("{host}: {err}");
                continue;
            }
        };
        if addrs.is_empty() {
            last_dns = format!("{host}: no address");
            continue;
        }
        last_dns = format!("{host} -> {}", addrs[0]);
        for addr in addrs {
            let ip = match addr {
                SocketAddr::V4(v) => SocketAddr::V4(v),
                SocketAddr::V6(_) => continue,
            };
            for &port in IOTC_QUERY_PORTS {
                let dest = SocketAddr::new(ip.ip(), port);
                if socket.send_to(&query, dest).is_ok() {
                    sent += 1;
                }
            }
        }
    }
    if sent == 0 {
        return Err(CameraError::Message(format!(
            "TUTK IOTC masters unreachable ({last_dns})"
        )));
    }
    let deadline = Instant::now() + Duration::from_secs(2);
    let mut buf = [0u8; 512];
    while Instant::now() < deadline {
        match socket.recv_from(&mut buf) {
            Ok((n, _from)) => {
                if n == 0 {
                    continue;
                }
                if let Some(peer) = parse_iotc_locate_reply(&buf[..n]) {
                    return Ok(peer);
                }
            }
            Err(err) if err.kind() == std::io::ErrorKind::WouldBlock => break,
            Err(err) if err.kind() == std::io::ErrorKind::TimedOut => break,
            Err(err) => return Err(err.into()),
        }
    }
    Err(CameraError::Message(format!(
        "TUTK IOTC uid {uid} not in master replies ({sent} probes, last {last_dns})"
    )))
}

/// UID (+ authkey) query copied by public Kalay clients onto IOTC masters.
pub fn iotc_uid_query(uid: &str, authkey: &str) -> Vec<u8> {
    let mut pkt = Vec::with_capacity(48);
    pkt.extend_from_slice(&[0x01, 0x00]);
    let uid_b = uid.as_bytes();
    let n = uid_b.len().min(20);
    pkt.extend_from_slice(&uid_b[..n]);
    pkt.resize(2 + 20, 0);
    let key_b = authkey.as_bytes();
    let n = key_b.len().min(8);
    pkt.extend_from_slice(&key_b[..n]);
    pkt.resize(2 + 20 + 8, 0);
    pkt
}

fn parse_iotc_locate_reply(buf: &[u8]) -> Option<IotcPeer> {
    if buf.len() < 8 {
        return None;
    }
    for window in buf.windows(6) {
        if window[0] == 0 && window[1] == 0 {
            continue;
        }
        let ip = Ipv4Addr::new(window[0], window[1], window[2], window[3]);
        if ip.is_unspecified() || ip.is_multicast() || ip.is_broadcast() || ip.is_loopback() {
            continue;
        }
        let port = u16::from_be_bytes([window[4], window[5]]);
        if port == 0 {
            continue;
        }
        return Some(IotcPeer { ip, port });
    }
    None
}

pub fn append_device_query(mut url: String, serial: &str) -> String {
    let serial = serial.trim();
    if serial.is_empty() {
        return url;
    }
    url.push_str("&device=");
    url.push_str(&percent_encode_query(serial));
    url
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn ttcode_403_is_auth_not_lan_toggle() {
        let msg = ttcode_error(&"cloud: cloud HTTP 403: The specified resource is forbidden.");
        assert!(msg.contains("403"));
        assert!(msg.contains("user-id"));
        assert!(!msg.contains("LAN Only"));
        assert!(!msg.contains("HTTP 405"));
    }

    #[test]
    fn ttcode_429_is_cloudflare_not_printer() {
        let msg = ttcode_error(
            &"cloud: cloud HTTP 429 (Cloudflare rate limit 1015). Wait a minute before Play; extra /ttcode retries make this worse.",
        );
        assert!(msg.contains("1015"));
        assert!(msg.contains("IP rate-limit"));
        assert!(!msg.contains("LAN Only"));
    }

    #[test]
    fn region_cn_uses_kalay_cn_masters() {
        assert_eq!(tutk_region("CN"), TutkRegion::Cn);
        assert!(iotc_masters("cn")
            .iter()
            .any(|h| h.contains("kalay.net.cn")));
        assert!(iotc_masters("us")
            .iter()
            .any(|h| *h == "m1.iotcplatform.com"));
    }

    #[test]
    fn uid_query_pads_twenty_and_eight() {
        let q = iotc_uid_query("01234567890ABCDEF012", "01234567");
        assert_eq!(q.len(), 30);
        assert_eq!(&q[0..2], &[0x01, 0x00]);
        assert_eq!(&q[2..22], b"01234567890ABCDEF012");
        assert_eq!(&q[22..30], b"01234567");
    }

    #[test]
    fn jpeg_sample_is_soi() {
        let jpeg = [0xFF, 0xD8, 0xFF, 0xD9];
        assert_eq!(jpeg_from_av_sample(&jpeg).unwrap(), &jpeg);
        assert!(!is_avc_sample(&jpeg));
    }

    #[test]
    fn mjpeg_frameinfo_prefix() {
        let mut buf = vec![0u8; 16];
        buf[0..2].copy_from_slice(&CODEC_MJPEG.to_le_bytes());
        buf.extend_from_slice(&[0xFF, 0xD8, 0x00]);
        assert_eq!(jpeg_from_av_sample(&buf).unwrap(), &[0xFF, 0xD8, 0x00][..]);
    }

    #[test]
    fn h264_frameinfo_is_avc() {
        let mut buf = vec![0u8; 16];
        buf[0..2].copy_from_slice(&CODEC_H264.to_le_bytes());
        buf.extend_from_slice(&[0x00, 0x00, 0x00, 0x01, 0x67]);
        assert!(is_avc_sample(&buf));
        assert!(jpeg_from_av_sample(&buf).is_none());
    }

    #[test]
    fn empty_uid_errors_before_udp() {
        let creds = CameraCreds {
            proto: CameraProto::Tutk,
            uid: String::new(),
            authkey: "k".into(),
            passwd: "p".into(),
            region: "us".into(),
            ..CameraCreds::default()
        };
        let err = for_each_jpeg(&creds, "", |_| true).unwrap_err();
        assert!(err.to_string().contains("uid"));
    }

    #[test]
    fn agora_is_not_jpeg_tutk() {
        let creds = CameraCreds {
            proto: CameraProto::Agora,
            authkey: "ak".into(),
            region: "us".into(),
            channel: "ch".into(),
            app_id: "app".into(),
            token: "tok".into(),
            ..CameraCreds::default()
        };
        let err = for_each_jpeg(&creds, "", |_| true).unwrap_err();
        assert!(err.to_string().contains("agora rtc"));
        assert!(err.to_string().contains("joinChannelEx"));
        let err = crate::agora::RtcSession::prepare(
            &crate::agora::AgoraJoin::from_creds(&creds).unwrap(),
        )
        .unwrap()
        .engine_steps();
        assert!(err.contains(&"joinChannelEx"));
        assert!(err.contains(&"registerVideoEncodedFrameObserver"));
    }

    #[test]
    fn locate_reply_picks_ipv4_port() {
        let mut buf = vec![0u8; 8];
        buf[0..4].copy_from_slice(&[192, 168, 1, 9]);
        buf[4..6].copy_from_slice(&32761u16.to_be_bytes());
        let peer = parse_iotc_locate_reply(&buf).unwrap();
        assert_eq!(peer.ip, Ipv4Addr::new(192, 168, 1, 9));
        assert_eq!(peer.port, 32761);
    }
}
