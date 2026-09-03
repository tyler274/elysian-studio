//! A1/P1 chamber JPEG over TLS TCP 6000 (OpenBambuAPI `video.md`).

use std::io::{Read, Write};
use std::net::TcpStream;
use std::time::Duration;

use rustls::{ClientConnection, StreamOwned};
use thiserror::Error;

use crate::mqtt::LAN_MQTT_USER;
use crate::tls::{lan_client_config, server_name, TlsError};
use bambu_device::Frame;

pub const LAN_CAMERA_PORT: u16 = 6000;

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

/// One JPEG frame from the P1/A1 TLS JPEG server.
pub fn snapshot_jpeg(host: &str, access_code: &str) -> Result<Vec<u8>, CameraError> {
    if access_code.is_empty() {
        return Err(CameraError::Message("LAN access code is empty".into()));
    }
    let config = lan_client_config()?;
    let tcp = TcpStream::connect((host, LAN_CAMERA_PORT))?;
    tcp.set_read_timeout(Some(Duration::from_secs(12)))?;
    tcp.set_write_timeout(Some(Duration::from_secs(8)))?;
    let name = server_name(host)?;
    let conn =
        ClientConnection::new(config, name).map_err(|err| CameraError::Message(err.to_string()))?;
    let mut tls = StreamOwned::new(conn, tcp);
    tls.write_all(&auth_packet(access_code))?;
    tls.flush()?;
    let mut header = [0u8; 16];
    tls.read_exact(&mut header)?;
    let jpeg_len = u32::from_le_bytes([header[0], header[1], header[2], header[3]]);
    if !(1000..=8_000_000).contains(&jpeg_len) {
        return Err(CameraError::Message(format!(
            "implausible JPEG size {jpeg_len} (X1/H2 use RTSPS :322, not this JPEG port)"
        )));
    }
    let mut jpeg = vec![0u8; jpeg_len as usize];
    tls.read_exact(&mut jpeg)?;
    if jpeg.len() < 4 || jpeg[0] != 0xFF || jpeg[1] != 0xD8 {
        return Err(CameraError::Message("payload is not JPEG SOI".into()));
    }
    Ok(jpeg)
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

#[cfg(test)]
mod tests {
    use super::*;

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
}
