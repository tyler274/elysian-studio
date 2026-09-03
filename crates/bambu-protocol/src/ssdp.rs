//! Bambu LAN SSDP (UDP 2021), ported from ClusterM/open-bamboo-networking `ssdp.cpp`.

use std::collections::BTreeMap;
use std::net::{Ipv4Addr, SocketAddr, SocketAddrV4, UdpSocket};
use std::time::{Duration, Instant};

use serde::{Deserialize, Serialize};
use thiserror::Error;

pub const SSDP_PORT: u16 = 2021;

#[derive(Debug, Error)]
pub enum SsdpError {
    #[error("io: {0}")]
    Io(#[from] std::io::Error),
}

#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct DiscoveredPrinter {
    pub dev_id: String,
    pub dev_ip: String,
    pub dev_name: String,
    pub dev_type: String,
    pub connect_type: String,
    pub bind_state: String,
}

pub fn parse_ssdp(data: &[u8]) -> Option<BTreeMap<String, String>> {
    let msg = std::str::from_utf8(data).ok()?;
    if !msg.contains("HTTP/1.") {
        return None;
    }
    let mut headers = BTreeMap::new();
    for line in msg.lines().skip(1) {
        let line = line.trim_end_matches('\r');
        if line.is_empty() {
            break;
        }
        let Some((k, v)) = line.split_once(':') else {
            continue;
        };
        headers.insert(k.trim().to_ascii_lowercase(), v.trim().to_string());
    }
    if headers.is_empty() {
        None
    } else {
        Some(headers)
    }
}

pub fn printer_from_headers(
    headers: &BTreeMap<String, String>,
    src_ip: Option<Ipv4Addr>,
) -> Option<DiscoveredPrinter> {
    let dev_id = headers.get("usn")?.clone();
    let dev_type = headers.get("devmodel.bambu.com")?.clone();
    if dev_id.is_empty() || dev_type.is_empty() {
        return None;
    }
    let dev_ip = headers
        .get("location")
        .cloned()
        .filter(|s| !s.is_empty())
        .or_else(|| src_ip.map(|ip| ip.to_string()))
        .unwrap_or_default();
    Some(DiscoveredPrinter {
        dev_id,
        dev_ip,
        dev_name: headers
            .get("devname.bambu.com")
            .cloned()
            .unwrap_or_default(),
        dev_type,
        connect_type: headers
            .get("devconnect.bambu.com")
            .cloned()
            .unwrap_or_default(),
        bind_state: headers
            .get("devbind.bambu.com")
            .cloned()
            .unwrap_or_default(),
    })
}

const MSEARCH: &str = "M-SEARCH * HTTP/1.1\r\n\
Host: 239.255.255.250:1990\r\n\
Man: \"ssdp:discover\"\r\n\
ST: urn:bambulab-com:device:3dprinter:1\r\n\
MX: 3\r\n\
\r\n";

/// Listen for printer NOTIFY/M-SEARCH replies on UDP 2021.
pub fn discover(timeout: Duration) -> Result<Vec<DiscoveredPrinter>, SsdpError> {
    let socket = UdpSocket::bind(SocketAddrV4::new(Ipv4Addr::UNSPECIFIED, SSDP_PORT))?;
    socket.set_broadcast(true)?;
    socket.set_read_timeout(Some(Duration::from_millis(250)))?;
    let _ = socket.send_to(
        MSEARCH.as_bytes(),
        SocketAddrV4::new(Ipv4Addr::BROADCAST, SSDP_PORT),
    );
    let _ = socket.send_to(
        MSEARCH.as_bytes(),
        SocketAddr::from((Ipv4Addr::new(239, 255, 255, 250), 1990)),
    );

    let deadline = Instant::now() + timeout;
    let mut found = Vec::new();
    let mut buf = [0u8; 4096];
    while Instant::now() < deadline {
        match socket.recv_from(&mut buf) {
            Ok((n, src)) => {
                let Some(headers) = parse_ssdp(&buf[..n]) else {
                    continue;
                };
                let src_ip = match src {
                    SocketAddr::V4(v) => Some(*v.ip()),
                    SocketAddr::V6(_) => None,
                };
                if let Some(printer) = printer_from_headers(&headers, src_ip) {
                    if !found
                        .iter()
                        .any(|p: &DiscoveredPrinter| p.dev_id == printer.dev_id)
                    {
                        found.push(printer);
                    }
                }
            }
            Err(err)
                if err.kind() == std::io::ErrorKind::WouldBlock
                    || err.kind() == std::io::ErrorKind::TimedOut =>
            {
                continue;
            }
            Err(err) => return Err(err.into()),
        }
    }
    Ok(found)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_bambu_notify() {
        let pkt = b"NOTIFY * HTTP/1.1\r\n\
USN: 00M00A000000000\r\n\
Location: 192.168.1.42\r\n\
DevModel.bambu.com: C12\r\n\
DevName.bambu.com: p1s-desk\r\n\
DevConnect.bambu.com: lan\r\n\
DevBind.bambu.com: free\r\n\
\r\n";
        let h = parse_ssdp(pkt).unwrap();
        let p = printer_from_headers(&h, None).unwrap();
        assert_eq!(p.dev_id, "00M00A000000000");
        assert_eq!(p.dev_ip, "192.168.1.42");
        assert_eq!(p.dev_name, "p1s-desk");
        assert_eq!(p.dev_type, "C12");
    }
}
