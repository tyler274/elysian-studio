//! rustls HTTPS GET/POST/PUT used by HMS and the public Bambu cloud API.
//!
//! No plugin, no shipped PEMs. Live hosts are only contacted from CLI/UI.

use std::io::{Read, Write};
use std::net::TcpStream;
use std::time::Duration;

use rustls::pki_types::ServerName;
use rustls::{ClientConnection, Stream};
use thiserror::Error;

use crate::tls::{install_ring_provider, lan_client_config, TlsError};

#[derive(Debug, Error)]
pub enum HttpsError {
    #[error("https: {0}")]
    Message(String),
    #[error(transparent)]
    Tls(#[from] TlsError),
    #[error(transparent)]
    Io(#[from] std::io::Error),
}

#[derive(Debug, Clone)]
pub struct HttpsResponse {
    pub status: u16,
    #[allow(dead_code)]
    pub headers: Vec<(String, String)>,
    pub body: Vec<u8>,
}

impl HttpsResponse {
    pub fn body_text(&self) -> String {
        String::from_utf8_lossy(&self.body).into_owned()
    }
}

pub fn parse_https_url(url: &str) -> Result<(String, String), HttpsError> {
    let rest = url
        .strip_prefix("https://")
        .ok_or_else(|| HttpsError::Message(format!("not https: {url}")))?;
    let (host_port, path) = match rest.find('/') {
        Some(i) => (&rest[..i], &rest[i..]),
        None => (rest, "/"),
    };
    let host = host_port
        .split(':')
        .next()
        .unwrap_or(host_port)
        .trim()
        .to_string();
    if host.is_empty() {
        return Err(HttpsError::Message("https url missing host".into()));
    }
    let path = if path.is_empty() {
        "/".into()
    } else {
        path.to_string()
    };
    Ok((host, path))
}

pub fn request(
    method: &str,
    host: &str,
    path: &str,
    extra_headers: &[(&str, &str)],
    body: Option<&[u8]>,
) -> Result<HttpsResponse, HttpsError> {
    install_ring_provider();
    let config = lan_client_config()?;
    let name = ServerName::try_from(host.to_string())
        .map_err(|err| HttpsError::Message(format!("server name {host}: {err}")))?;
    let mut tcp = TcpStream::connect((host, 443))?;
    let timeout = if body.map(|b| b.len()).unwrap_or(0) > 256 * 1024 {
        Duration::from_secs(120)
    } else {
        Duration::from_secs(20)
    };
    tcp.set_read_timeout(Some(timeout))?;
    tcp.set_write_timeout(Some(timeout))?;
    let mut conn =
        ClientConnection::new(config, name).map_err(|err| HttpsError::Message(err.to_string()))?;
    let mut tls = Stream::new(&mut conn, &mut tcp);
    let path = if path.starts_with('/') {
        path.to_string()
    } else {
        format!("/{path}")
    };
    let mut req = format!("{method} {path} HTTP/1.1\r\nHost: {host}\r\nConnection: close\r\n");
    for (name, value) in extra_headers {
        req.push_str(&format!("{name}: {value}\r\n"));
    }
    if let Some(body) = body {
        if !extra_headers
            .iter()
            .any(|(n, _)| n.eq_ignore_ascii_case("content-length"))
        {
            req.push_str(&format!("Content-Length: {}\r\n", body.len()));
        }
        req.push_str("\r\n");
        tls.write_all(req.as_bytes())?;
        tls.write_all(body)?;
    } else {
        req.push_str("\r\n");
        tls.write_all(req.as_bytes())?;
    }
    tls.flush()?;
    let mut raw = Vec::new();
    tls.read_to_end(&mut raw)?;
    parse_http_response(&raw).ok_or_else(|| HttpsError::Message("HTTP response had no body".into()))
}

pub fn request_url(
    method: &str,
    url: &str,
    extra_headers: &[(&str, &str)],
    body: Option<&[u8]>,
) -> Result<HttpsResponse, HttpsError> {
    let (host, path) = parse_https_url(url)?;
    request(method, &host, &path, extra_headers, body)
}

pub fn parse_http_response(raw: &[u8]) -> Option<HttpsResponse> {
    let split = raw.windows(4).position(|w| w == b"\r\n\r\n")?;
    let header_bytes = &raw[..split];
    let rest = &raw[split + 4..];
    let headers_text = String::from_utf8_lossy(header_bytes);
    let mut lines = headers_text.lines();
    let status_line = lines.next().unwrap_or("");
    let status = status_line
        .split_whitespace()
        .nth(1)
        .and_then(|s| s.parse().ok())
        .unwrap_or(0);
    let mut headers = Vec::new();
    for line in lines {
        let Some((name, value)) = line.split_once(':') else {
            continue;
        };
        headers.push((name.trim().to_string(), value.trim().to_string()));
    }
    let chunked = headers.iter().any(|(n, v)| {
        n.eq_ignore_ascii_case("transfer-encoding") && v.to_ascii_lowercase().contains("chunked")
    });
    let body = if chunked {
        decode_chunked(rest)?
    } else if let Some((_, len)) = headers
        .iter()
        .find(|(n, _)| n.eq_ignore_ascii_case("content-length"))
    {
        let n: usize = len.parse().ok()?;
        rest.get(..n.min(rest.len()))?.to_vec()
    } else {
        rest.to_vec()
    };
    Some(HttpsResponse {
        status,
        headers,
        body,
    })
}

fn decode_chunked(body: &[u8]) -> Option<Vec<u8>> {
    let mut rest = body;
    let mut out = Vec::new();
    loop {
        let nl = rest.windows(2).position(|w| w == b"\r\n")?;
        let size_line = std::str::from_utf8(&rest[..nl]).ok()?.trim();
        let size = usize::from_str_radix(size_line.split(';').next().unwrap_or(""), 16).ok()?;
        rest = &rest[nl + 2..];
        if size == 0 {
            break;
        }
        if rest.len() < size {
            return None;
        }
        out.extend_from_slice(&rest[..size]);
        rest = rest.get(size..)?;
        rest = rest.strip_prefix(b"\r\n").unwrap_or(rest);
    }
    Some(out)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_https_url() {
        let (host, path) =
            parse_https_url("https://api.bambulab.com/v1/iot-service/api/user/bind").unwrap();
        assert_eq!(host, "api.bambulab.com");
        assert_eq!(path, "/v1/iot-service/api/user/bind");
    }

    #[test]
    fn parses_chunked_200() {
        let raw = b"HTTP/1.1 200 OK\r\nTransfer-Encoding: chunked\r\n\r\n5\r\nhello\r\n6\r\n world\r\n0\r\n\r\n";
        let resp = parse_http_response(raw).unwrap();
        assert_eq!(resp.status, 200);
        assert_eq!(resp.body_text(), "hello world");
    }

    #[test]
    fn parses_content_length() {
        let raw = b"HTTP/1.1 201 Created\r\nContent-Length: 4\r\n\r\nabcdXXXX";
        let resp = parse_http_response(raw).unwrap();
        assert_eq!(resp.status, 201);
        assert_eq!(resp.body, b"abcd");
    }
}
