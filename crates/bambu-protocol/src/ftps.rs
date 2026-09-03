//! Implicit FTPS (TCP/990) matching ClusterM `obn::ftps` printer quirks:
//! PASV IP rewrite, delayed data-channel TLS after `150`, PROT P.

use std::io::{Read, Write};
use std::net::TcpStream;
use std::sync::Arc;
use std::time::Duration;

use rustls::{ClientConfig, ClientConnection, StreamOwned};
use thiserror::Error;

use crate::mqtt::LAN_MQTT_USER;
use crate::tls::{lan_client_config, server_name, TlsError};

pub const LAN_FTPS_PORT: u16 = 990;

#[derive(Debug, Error)]
pub enum FtpsError {
    #[error("ftps: {0}")]
    Message(String),
    #[error(transparent)]
    Tls(#[from] TlsError),
    #[error("io: {0}")]
    Io(#[from] std::io::Error),
}

struct Control {
    tls: StreamOwned<ClientConnection, TcpStream>,
    buf: Vec<u8>,
}

impl Control {
    fn cmd(&mut self, line: &str) -> Result<(u16, String), FtpsError> {
        self.tls.write_all(line.as_bytes())?;
        self.tls.write_all(b"\r\n")?;
        self.tls.flush()?;
        self.read_reply()
    }

    fn read_line(&mut self) -> Result<String, FtpsError> {
        loop {
            if let Some(i) = self.buf.iter().position(|&b| b == b'\n') {
                let mut line = self.buf.drain(..=i).collect::<Vec<_>>();
                if line.ends_with(b"\n") {
                    line.pop();
                }
                if line.ends_with(b"\r") {
                    line.pop();
                }
                return Ok(String::from_utf8_lossy(&line).into_owned());
            }
            let mut tmp = [0u8; 1024];
            let n = self.tls.read(&mut tmp)?;
            if n == 0 {
                return Err(FtpsError::Message("control closed".into()));
            }
            self.buf.extend_from_slice(&tmp[..n]);
        }
    }

    fn read_reply(&mut self) -> Result<(u16, String), FtpsError> {
        let first = self.read_line()?;
        if first.len() < 4 || !first.as_bytes()[..3].iter().all(u8::is_ascii_digit) {
            return Err(FtpsError::Message(format!("bad ftp reply: {first}")));
        }
        let code: u16 = first[..3]
            .parse()
            .map_err(|_| FtpsError::Message(format!("bad ftp code: {first}")))?;
        let mut body = vec![strip_code(&first)];
        if first.as_bytes()[3] == b'-' {
            loop {
                let line = self.read_line()?;
                let done = line.len() >= 4
                    && line.as_bytes()[..3].iter().all(u8::is_ascii_digit)
                    && line.as_bytes()[3] == b' '
                    && line[..3] == first[..3];
                body.push(strip_code(&line));
                if done {
                    break;
                }
            }
        }
        Ok((code, body.join("\n")))
    }
}

fn strip_code(line: &str) -> String {
    if line.len() >= 4 && line.as_bytes()[..3].iter().all(u8::is_ascii_digit) {
        line[4..].to_string()
    } else {
        line.to_string()
    }
}

/// Parse `227 Entering Passive Mode (h1,h2,h3,h4,p1,p2)` and return the data port.
pub fn parse_pasv_port(body: &str) -> Option<u16> {
    let start = body.find('(')?;
    let end = body[start + 1..].find(')')? + start + 1;
    let inner = &body[start + 1..end];
    let parts: Vec<&str> = inner.split(',').map(str::trim).collect();
    if parts.len() != 6 {
        return None;
    }
    let p1: u16 = parts[4].parse().ok()?;
    let p2: u16 = parts[5].parse().ok()?;
    Some(p1 * 256 + p2)
}

fn connect_tls(
    host: &str,
    port: u16,
    config: Arc<ClientConfig>,
    timeout: Duration,
) -> Result<StreamOwned<ClientConnection, TcpStream>, FtpsError> {
    let tcp = TcpStream::connect((host, port))?;
    tcp.set_read_timeout(Some(timeout))?;
    tcp.set_write_timeout(Some(timeout))?;
    let name = server_name(host)?;
    let conn =
        ClientConnection::new(config, name).map_err(|err| FtpsError::Message(err.to_string()))?;
    Ok(StreamOwned::new(conn, tcp))
}

/// Upload `bytes` to `remote_name` at the FTPS root (`STOR`).
pub fn stor(
    host: &str,
    access_code: &str,
    remote_name: &str,
    bytes: &[u8],
) -> Result<(), FtpsError> {
    let config = lan_client_config()?;
    let mut ctrl = Control {
        tls: connect_tls(host, LAN_FTPS_PORT, config.clone(), Duration::from_secs(15))?,
        buf: Vec::new(),
    };
    let (code, body) = ctrl.read_reply()?;
    if code != 220 {
        return Err(FtpsError::Message(format!("no 220 banner: {code} {body}")));
    }
    let (code, body) = ctrl.cmd(&format!("USER {LAN_MQTT_USER}"))?;
    if code == 331 {
        let (code, body) = ctrl.cmd(&format!("PASS {access_code}"))?;
        if code != 230 {
            return Err(FtpsError::Message(format!("login rejected: {code} {body}")));
        }
    } else if code != 230 {
        return Err(FtpsError::Message(format!("USER rejected: {code} {body}")));
    }
    for line in ["TYPE I", "PBSZ 0", "PROT P"] {
        let (code, body) = ctrl.cmd(line)?;
        if code != 200 {
            return Err(FtpsError::Message(format!(
                "{line} rejected: {code} {body}"
            )));
        }
    }

    let (code, body) = ctrl.cmd("PASV")?;
    if code != 227 {
        return Err(FtpsError::Message(format!("PASV rejected: {code} {body}")));
    }
    let data_port = parse_pasv_port(&body)
        .ok_or_else(|| FtpsError::Message(format!("PASV unparseable: {body}")))?;
    // Ignore the PASV IP (often 0.0.0.0) and reconnect to the control host.
    let data_tcp = TcpStream::connect((host, data_port))?;
    data_tcp.set_read_timeout(Some(Duration::from_secs(120)))?;
    data_tcp.set_write_timeout(Some(Duration::from_secs(120)))?;

    let (code, body) = ctrl.cmd(&format!("STOR {remote_name}"))?;
    if code != 150 && code != 125 {
        return Err(FtpsError::Message(format!("STOR rejected: {code} {body}")));
    }

    let mut data = connect_tls_on(host, config, data_tcp)?;
    data.write_all(bytes)?;
    data.flush()?;
    data.conn.send_close_notify();
    let _ = data.flush();
    drop(data);

    let (code, body) = ctrl.read_reply()?;
    if code != 226 && code != 250 {
        return Err(FtpsError::Message(format!(
            "STOR incomplete: {code} {body}"
        )));
    }
    let _ = ctrl.cmd("QUIT");
    Ok(())
}

fn connect_tls_on(
    host: &str,
    config: Arc<ClientConfig>,
    tcp: TcpStream,
) -> Result<StreamOwned<ClientConnection, TcpStream>, FtpsError> {
    let name = server_name(host)?;
    let conn =
        ClientConnection::new(config, name).map_err(|err| FtpsError::Message(err.to_string()))?;
    Ok(StreamOwned::new(conn, tcp))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_pasv() {
        assert_eq!(
            parse_pasv_port("Entering Passive Mode (0,0,0,0,20,80)"),
            Some(20 * 256 + 80)
        );
        assert_eq!(
            parse_pasv_port("227 Entering Passive Mode (192,168,1,9,195,80)."),
            Some(195 * 256 + 80)
        );
        assert_eq!(parse_pasv_port("no tuple"), None);
    }
}
