//! A1/P1 chamber JPEG over TLS TCP 6000 (OpenBambuAPI `video.md`).

use std::io::{Read, Write};
use std::net::{TcpStream, ToSocketAddrs};
use std::time::Duration;

use rustls::{ClientConnection, StreamOwned};
use thiserror::Error;

use crate::mqtt::LAN_MQTT_USER;
use crate::tls::{lan_client_config, server_name, TlsError};
use bambu_device::Frame;

pub const LAN_CAMERA_PORT: u16 = 6000;
/// X1 / H2 chamber is RTSPS on TCP 322 (not the P1/A1 JPEG port).
pub const LAN_RTSPS_PORT: u16 = 322;

const JPEG_MIN: u32 = 1000;
const JPEG_MAX: u32 = 8_000_000;

#[derive(Debug, Error)]
pub enum CameraError {
    #[error("camera: {0}")]
    Message(String),
    #[error(transparent)]
    Tls(#[from] TlsError),
    #[error("io: {0}")]
    Io(#[from] std::io::Error),
}

pub fn auth_packet(access_code: &str) -> [u8; 80] {
    let mut pkt = [0u8; 80];
    pkt[0..4].copy_from_slice(&0x40u32.to_le_bytes());
    pkt[4..8].copy_from_slice(&0x3000u32.to_le_bytes());
    let user = LAN_MQTT_USER.as_bytes();
    let n = user.len().min(32);
    pkt[16..16 + n].copy_from_slice(&user[..n]);
    let code = access_code.as_bytes();
    let n = code.len().min(32);
    pkt[48..48 + n].copy_from_slice(&code[..n]);
    pkt
}

/// Little-endian payload length in the 16-byte JPEG stream header.
pub fn jpeg_payload_len(header: &[u8; 16]) -> Result<u32, CameraError> {
    let jpeg_len = u32::from_le_bytes([header[0], header[1], header[2], header[3]]);
    if !(JPEG_MIN..=JPEG_MAX).contains(&jpeg_len) {
        return Err(CameraError::Message(format!(
            "implausible JPEG size {jpeg_len} (X1/H2 use RTSPS :322, not this JPEG port)"
        )));
    }
    Ok(jpeg_len)
}

/// One header + JPEG payload from an already-authenticated TLS stream.
pub fn read_jpeg_frame<R: Read>(tls: &mut R) -> Result<Vec<u8>, CameraError> {
    let mut header = [0u8; 16];
    tls.read_exact(&mut header)?;
    let jpeg_len = jpeg_payload_len(&header)?;
    let mut jpeg = vec![0u8; jpeg_len as usize];
    tls.read_exact(&mut jpeg)?;
    if jpeg.len() < 4 || jpeg[0] != 0xFF || jpeg[1] != 0xD8 {
        return Err(CameraError::Message("payload is not JPEG SOI".into()));
    }
    Ok(jpeg)
}

fn tcp_connect(host: &str, port: u16, timeout: Duration) -> Result<TcpStream, CameraError> {
    let addr = (host, port)
        .to_socket_addrs()?
        .next()
        .ok_or_else(|| CameraError::Message(format!("no address for {host}:{port}")))?;
    Ok(TcpStream::connect_timeout(&addr, timeout)?)
}

fn tls_connect(
    host: &str,
    port: u16,
) -> Result<StreamOwned<ClientConnection, TcpStream>, CameraError> {
    let config = lan_client_config()?;
    let tcp = tcp_connect(host, port, Duration::from_secs(5))?;
    tcp.set_read_timeout(Some(Duration::from_secs(12)))?;
    tcp.set_write_timeout(Some(Duration::from_secs(8)))?;
    let name = server_name(host)?;
    let conn =
        ClientConnection::new(config, name).map_err(|err| CameraError::Message(err.to_string()))?;
    Ok(StreamOwned::new(conn, tcp))
}

/// Authenticated P1/A1 JPEG stream. Keep the connection and call [`JpegStream::next_jpeg`].
pub struct JpegStream {
    tls: StreamOwned<ClientConnection, TcpStream>,
}

impl JpegStream {
    pub fn connect(host: &str, access_code: &str) -> Result<Self, CameraError> {
        if access_code.is_empty() {
            return Err(CameraError::Message("LAN access code is empty".into()));
        }
        let mut tls = tls_connect(host, LAN_CAMERA_PORT)?;
        tls.write_all(&auth_packet(access_code))?;
        tls.flush()?;
        Ok(Self { tls })
    }

    pub fn next_jpeg(&mut self) -> Result<Vec<u8>, CameraError> {
        read_jpeg_frame(&mut self.tls)
    }
}

/// One JPEG frame from the P1/A1 TLS JPEG server.
pub fn snapshot_jpeg(host: &str, access_code: &str) -> Result<Vec<u8>, CameraError> {
    JpegStream::connect(host, access_code)?.next_jpeg()
}

pub fn jpeg_to_frame(jpeg: &[u8]) -> Result<Frame, CameraError> {
    let mut decoder = jpeg_decoder::Decoder::new(jpeg);
    let pixels = decoder
        .decode()
        .map_err(|err| CameraError::Message(err.to_string()))?;
    let info = decoder
        .info()
        .ok_or_else(|| CameraError::Message("JPEG has no frame info".into()))?;
    let rgba = match info.pixel_format {
        jpeg_decoder::PixelFormat::RGB24 => pixels
            .as_chunks::<3>()
            .0
            .iter()
            .flat_map(|c| [c[0], c[1], c[2], 255])
            .collect(),
        jpeg_decoder::PixelFormat::L8 => pixels.iter().flat_map(|&y| [y, y, y, 255]).collect(),
        jpeg_decoder::PixelFormat::L16 => pixels
            .as_chunks::<2>()
            .0
            .iter()
            .flat_map(|c| {
                let y = c[1];
                [y, y, y, 255]
            })
            .collect(),
        jpeg_decoder::PixelFormat::CMYK32 => {
            return Err(CameraError::Message("unsupported JPEG CMYK".into()));
        }
    };
    Ok(Frame {
        width: info.width as u32,
        height: info.height as u32,
        rgba,
    })
}

pub fn snapshot_frame(host: &str, access_code: &str) -> Result<Frame, CameraError> {
    jpeg_to_frame(&snapshot_jpeg(host, access_code)?)
}

/// C++ `rtsps://bblp:<code>@<ip>:322/streaming/live/1`.
pub fn rtsps_url(host: &str, access_code: &str) -> String {
    format!("rtsps://{LAN_MQTT_USER}:{access_code}@{host}:{LAN_RTSPS_PORT}/streaming/live/1")
}

/// TLS OPTIONS + DESCRIBE on :322. Does not decode H.264 and never ships Bambu PEMs.
pub fn probe_rtsps(host: &str, access_code: &str) -> Result<String, CameraError> {
    describe_rtsps(host, access_code).map(|live| live.sdp)
}

#[derive(Debug, Clone)]
pub struct RtspsSession {
    pub url: String,
    pub sdp: String,
}

/// OPTIONS then DESCRIBE on one TLS connection (live view handshake without PEMs).
pub fn describe_rtsps(host: &str, access_code: &str) -> Result<RtspsSession, CameraError> {
    if access_code.is_empty() {
        return Err(CameraError::Message("LAN access code is empty".into()));
    }
    let mut tls = tls_connect(host, LAN_RTSPS_PORT)?;
    let options = format!(
        "OPTIONS rtsp://{host}:{LAN_RTSPS_PORT}/streaming/live/1 RTSP/1.0\r\nCSeq: 1\r\nUser-Agent: bambu-studio-rs\r\n\r\n"
    );
    tls.write_all(options.as_bytes())?;
    tls.flush()?;
    let mut buf = [0u8; 4096];
    let n = tls.read(&mut buf)?;
    if n == 0 {
        return Err(CameraError::Message("empty RTSPS OPTIONS response".into()));
    }
    let describe = format!(
        "DESCRIBE rtsp://{host}:{LAN_RTSPS_PORT}/streaming/live/1 RTSP/1.0\r\nCSeq: 2\r\nAccept: application/sdp\r\nUser-Agent: bambu-studio-rs\r\n\r\n"
    );
    tls.write_all(describe.as_bytes())?;
    tls.flush()?;
    let mut sdp_buf = [0u8; 8192];
    let m = tls.read(&mut sdp_buf).unwrap_or(0);
    let mut sdp = String::from_utf8_lossy(&buf[..n]).to_string();
    if m > 0 {
        sdp.push('\n');
        sdp.push_str(&String::from_utf8_lossy(&sdp_buf[..m]));
    }
    if sdp.trim().is_empty() {
        return Err(CameraError::Message("empty RTSPS DESCRIBE response".into()));
    }
    Ok(RtspsSession {
        url: rtsps_url(host, access_code),
        sdp,
    })
}

#[derive(Debug)]
pub enum ChamberCapture {
    Jpeg(Vec<u8>),
    Rtsps { url: String, options: String },
}

/// P1/A1 JPEG :6000, then X1/H2 RTSPS :322 OPTIONS+DESCRIBE.
pub fn capture_chamber(host: &str, access_code: &str) -> Result<ChamberCapture, CameraError> {
    match snapshot_jpeg(host, access_code) {
        Ok(jpeg) => Ok(ChamberCapture::Jpeg(jpeg)),
        Err(jpeg_err) => match describe_rtsps(host, access_code) {
            Ok(live) => Ok(ChamberCapture::Rtsps {
                url: live.url,
                options: live.sdp,
            }),
            Err(rtsps_err) => Err(CameraError::Message(format!(
                "JPEG :{LAN_CAMERA_PORT}: {jpeg_err}; RTSPS :{LAN_RTSPS_PORT}: {rtsps_err}"
            ))),
        },
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::Cursor;

    fn fake_jpeg(len: usize) -> Vec<u8> {
        let mut jpeg = vec![0u8; len];
        jpeg[0] = 0xFF;
        jpeg[1] = 0xD8;
        jpeg[len - 2] = 0xFF;
        jpeg[len - 1] = 0xD9;
        jpeg
    }

    fn framed(jpeg: &[u8]) -> Vec<u8> {
        let mut header = [0u8; 16];
        header[0..4].copy_from_slice(&(jpeg.len() as u32).to_le_bytes());
        let mut out = header.to_vec();
        out.extend_from_slice(jpeg);
        out
    }

    #[test]
    fn auth_packet_layout() {
        let p = auth_packet("12345678");
        assert_eq!(&p[0..4], &0x40u32.to_le_bytes());
        assert_eq!(&p[4..8], &0x3000u32.to_le_bytes());
        assert_eq!(&p[16..20], b"bblp");
        assert_eq!(&p[48..56], b"12345678");
        assert_eq!(p[20], 0);
        assert_eq!(p[56], 0);
    }

    #[test]
    fn rtsps_url_uses_lan_user_and_port_322() {
        let url = rtsps_url("192.168.1.10", "12345678");
        assert!(url.starts_with("rtsps://bblp:12345678@192.168.1.10:322/"));
        assert!(url.contains("/streaming/live/1"));
    }

    #[test]
    fn jpeg_payload_len_rejects_implausible_size() {
        let mut header = [0u8; 16];
        header[0..4].copy_from_slice(&40u32.to_le_bytes());
        assert!(jpeg_payload_len(&header).is_err());
    }

    #[test]
    fn read_jpeg_frame_reads_header_then_payload() {
        let jpeg = fake_jpeg(2048);
        let mut cursor = Cursor::new(framed(&jpeg));
        let out = read_jpeg_frame(&mut cursor).unwrap();
        assert_eq!(out, jpeg);
    }

    #[test]
    fn read_jpeg_frame_loops_two_headers() {
        let a = fake_jpeg(1024);
        let b = fake_jpeg(2048);
        let mut bytes = framed(&a);
        bytes.extend_from_slice(&framed(&b));
        let mut cursor = Cursor::new(bytes);
        assert_eq!(read_jpeg_frame(&mut cursor).unwrap(), a);
        assert_eq!(read_jpeg_frame(&mut cursor).unwrap(), b);
    }

    #[test]
    fn jpeg_to_frame_two_payloads_two_rgba_sizes() {
        let a = jpeg_to_frame(include_bytes!("testdata/jpeg_8x8.jpg")).unwrap();
        let b = jpeg_to_frame(include_bytes!("testdata/jpeg_16x8.jpg")).unwrap();
        assert_eq!((a.width, a.height), (8, 8));
        assert_eq!(a.rgba.len(), 8 * 8 * 4);
        assert_eq!((b.width, b.height), (16, 8));
        assert_eq!(b.rgba.len(), 16 * 8 * 4);
    }
}
