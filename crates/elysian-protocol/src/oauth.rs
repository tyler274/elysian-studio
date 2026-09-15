//! Studio-style cloud login: system browser + localhost ticket callback.
//!
//! Matches C++ `HttpServer` (`127.0.0.1:13618`) and
//! `POST /v1/user-service/user/ticket/{ticket}` from open-bambu-networking.
//! Live HTTPS stays in CLI/UI; this module's tests are local/fixture only.

use std::io::{Read, Write};
use std::net::{TcpListener, TcpStream};
use std::process::Command;
use std::thread;
use std::time::{Duration, Instant};

use crate::cloud_api::{CloudApi, CloudApiError, LoginResult};

/// C++ `LOCALHOST_PORT` in `slic3r/GUI/HttpServer.hpp`.
pub const OAUTH_CALLBACK_PORT: u16 = 13618;

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct OAuthCallback {
    pub ticket: String,
    pub redirect_url: String,
}

pub fn web_host(region: &str) -> &'static str {
    match region.trim().to_ascii_lowercase().as_str() {
        "cn" | "china" => "https://bambulab.cn",
        _ => "https://bambulab.com",
    }
}

/// Studio system-browser login (`WebUserLoginDialog` + `get_localhost_url`).
/// After the portal session exists, `/sign-in/callback?...&slicerLoginType=ticket`
/// 302s to `callback` with `?ticket=`.
pub fn sign_in_url(region: &str, callback: &str) -> String {
    let host = web_host(region);
    let prefix = match region.trim().to_ascii_lowercase().as_str() {
        "cn" | "china" => "",
        _ => "/en",
    };
    let redirect = percent_encode_query(callback.trim_end_matches('/'));
    let inner = format!(
        "{host}{prefix}/sign-in/callback?source=portal&locale=en&redirect_url={redirect}&openBy=suite&from=studio&slicerLoginType=ticket"
    );
    format!(
        "{host}{prefix}/sign-in?from=studio&source=portal&to={}",
        percent_encode_query(&inner)
    )
}

pub fn oauth_callback_url() -> String {
    format!("http://localhost:{OAUTH_CALLBACK_PORT}/")
}

pub fn percent_encode_query(s: &str) -> String {
    let mut out = String::new();
    for b in s.bytes() {
        match b {
            b'A'..=b'Z' | b'a'..=b'z' | b'0'..=b'9' | b'-' | b'_' | b'.' | b'~' => {
                out.push(b as char);
            }
            _ => out.push_str(&format!("%{b:02X}")),
        }
    }
    out
}

pub fn percent_decode(s: &str) -> String {
    let bytes = s.as_bytes();
    let mut out = Vec::with_capacity(bytes.len());
    let mut i = 0;
    while i < bytes.len() {
        if bytes[i] == b'%' && i + 2 < bytes.len() {
            if let (Some(hi), Some(lo)) = (from_hex(bytes[i + 1]), from_hex(bytes[i + 2])) {
                out.push((hi << 4) | lo);
                i += 3;
                continue;
            }
        }
        if bytes[i] == b'+' {
            out.push(b' ');
        } else {
            out.push(bytes[i]);
        }
        i += 1;
    }
    String::from_utf8_lossy(&out).into_owned()
}

fn from_hex(b: u8) -> Option<u8> {
    match b {
        b'0'..=b'9' => Some(b - b'0'),
        b'a'..=b'f' => Some(b - b'a' + 10),
        b'A'..=b'F' => Some(b - b'A' + 10),
        _ => None,
    }
}

/// Parse `GET /path?ticket=…&redirect_url=…` (C++ `parse_login_params`).
pub fn parse_oauth_callback_target(target: &str) -> Option<OAuthCallback> {
    let query = target.split_once('?').map(|(_, q)| q).unwrap_or(target);
    let query = query.split('#').next().unwrap_or(query);
    let mut ticket = String::new();
    let mut redirect_url = String::new();
    for pair in query.split('&') {
        let (k, v) = pair.split_once('=').unwrap_or((pair, ""));
        let key = percent_decode(k);
        let val = percent_decode(v);
        match key.as_str() {
            "ticket" | "code" => {
                if ticket.is_empty() {
                    ticket = val;
                }
            }
            "redirect_url" | "redirect" => redirect_url = val,
            _ => {}
        }
    }
    if ticket.is_empty() {
        return None;
    }
    Some(OAuthCallback {
        ticket,
        redirect_url,
    })
}

pub fn open_default_browser(url: &str) -> Result<(), CloudApiError> {
    let candidates = ["xdg-open", "gio", "firefox", "chromium"];
    for bin in candidates {
        let args: Vec<&str> = if bin == "gio" {
            vec!["open", url]
        } else {
            vec![url]
        };
        match Command::new(bin).args(&args).spawn() {
            Ok(_) => return Ok(()),
            Err(err) if err.kind() == std::io::ErrorKind::NotFound => continue,
            Err(err) => {
                return Err(CloudApiError::Message(format!("open browser: {err}")));
            }
        }
    }
    Err(CloudApiError::Message(
        "no browser launcher (xdg-open / gio / firefox)".into(),
    ))
}

/// Studio `HttpServer::start` on `127.0.0.1:13618`, also `::1` when available.
pub fn wait_for_oauth_callback(timeout: Duration) -> Result<OAuthCallback, CloudApiError> {
    let v4 = TcpListener::bind(("127.0.0.1", OAUTH_CALLBACK_PORT)).map_err(|err| {
        CloudApiError::Message(format!(
            "cannot bind 127.0.0.1:{OAUTH_CALLBACK_PORT} ({err}); is Bambu Studio already listening?"
        ))
    })?;
    let v6 = TcpListener::bind(("::1", OAUTH_CALLBACK_PORT)).ok();
    accept_oauth_callback_from(&v4, v6.as_ref(), timeout)
}

pub fn accept_oauth_callback_from(
    v4: &TcpListener,
    v6: Option<&TcpListener>,
    timeout: Duration,
) -> Result<OAuthCallback, CloudApiError> {
    v4.set_nonblocking(true)?;
    if let Some(l) = v6 {
        l.set_nonblocking(true)?;
    }
    let deadline = Instant::now() + timeout;
    loop {
        if let Some(cb) = try_accept(v4)? {
            return Ok(cb);
        }
        if let Some(l) = v6 {
            if let Some(cb) = try_accept(l)? {
                return Ok(cb);
            }
        }
        if Instant::now() >= deadline {
            return Err(CloudApiError::Message(
                "timed out waiting for browser OAuth callback".into(),
            ));
        }
        thread::sleep(Duration::from_millis(50));
    }
}

fn try_accept(listener: &TcpListener) -> Result<Option<OAuthCallback>, CloudApiError> {
    match listener.accept() {
        Ok((stream, _)) => handle_oauth_conn(stream),
        Err(err) if err.kind() == std::io::ErrorKind::WouldBlock => Ok(None),
        Err(err) => Err(CloudApiError::Message(format!("oauth accept: {err}"))),
    }
}

fn handle_oauth_conn(mut stream: TcpStream) -> Result<Option<OAuthCallback>, CloudApiError> {
    stream.set_nonblocking(false)?;
    stream.set_read_timeout(Some(Duration::from_secs(5)))?;
    let mut buf = Vec::new();
    let mut tmp = [0u8; 1024];
    loop {
        match stream.read(&mut tmp) {
            Ok(0) => break,
            Ok(n) => {
                buf.extend_from_slice(&tmp[..n]);
                if buf.windows(4).any(|w| w == b"\r\n\r\n") || buf.len() > 16 * 1024 {
                    break;
                }
            }
            Err(err) if err.kind() == std::io::ErrorKind::WouldBlock => break,
            Err(err) if err.kind() == std::io::ErrorKind::TimedOut => break,
            Err(err) => {
                return Err(CloudApiError::Message(format!("oauth read: {err}")));
            }
        }
    }
    let text = String::from_utf8_lossy(&buf);
    let target = text
        .lines()
        .next()
        .and_then(|line| line.split_whitespace().nth(1))
        .unwrap_or("/");
    let Some(cb) = parse_oauth_callback_target(target) else {
        let _ = stream.write_all(
            b"HTTP/1.1 404 Not Found\r\nContent-Type: text/html\r\nConnection: close\r\n\r\n<html><body><h1>404</h1></body></html>",
        );
        return Ok(None);
    };
    write_oauth_success(&mut stream)?;
    Ok(Some(cb))
}

/// Local success page. Stock Studio 302s to `redirect_url?result=success`, and
/// that portal page then opens `bambustudioopen://` (Mac/deep-link). Without a
/// registered handler, KDE KIO shows "Could not read file bambustudioopen://.".
fn write_oauth_success(stream: &mut TcpStream) -> Result<(), CloudApiError> {
    let body = "<!DOCTYPE html><html><head><meta charset=\"utf-8\"><title>Signed in</title></head><body><h1>Signed in</h1><p>You can close this tab and return to Elysian Studio.</p></body></html>";
    let resp = format!(
        "HTTP/1.1 200 OK\r\nContent-Type: text/html; charset=utf-8\r\nConnection: close\r\nContent-Length: {}\r\n\r\n{body}",
        body.len()
    );
    stream.write_all(resp.as_bytes())?;
    Ok(())
}

/// Open the Bambu sign-in portal, wait for the localhost ticket, exchange it.
pub fn oauth_login(
    region: &str,
    open_browser: bool,
    timeout: Duration,
) -> Result<LoginResult, CloudApiError> {
    let callback = oauth_callback_url();
    let url = sign_in_url(region, &callback);
    if open_browser {
        open_default_browser(&url)?;
    }
    tracing::info!("oauth: waiting for ticket on {}", callback);
    let cb = wait_for_oauth_callback(timeout)?;
    CloudApi::login_with_ticket(region, &cb.ticket)
}

/// Exchange a ticket already copied from the callback URL.
pub fn login_with_ticket(region: &str, ticket: &str) -> Result<LoginResult, CloudApiError> {
    CloudApi::login_with_ticket(region, ticket)
}

pub fn persist_login(
    dir: impl AsRef<std::path::Path>,
    region: &str,
    result: LoginResult,
) -> Result<crate::CloudSession, CloudApiError> {
    match result {
        LoginResult::NeedsCode { login_type } => Err(CloudApiError::Message(format!(
            "oauth ticket returned loginType {login_type}"
        ))),
        LoginResult::Tokens {
            access_token,
            refresh_token,
            user_id,
        } => crate::cloud::store_login_tokens(dir, region, access_token, refresh_token, user_id)
            .map_err(|err| CloudApiError::Message(err.to_string())),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::Write;

    #[test]
    fn sign_in_urls() {
        let us = sign_in_url("us", "http://localhost:13618/");
        assert!(us.starts_with("https://bambulab.com/en/sign-in?from=studio&source=portal&to="));
        assert!(us.contains("slicerLoginType"));
        let cn = sign_in_url("cn", "http://localhost:13618/");
        assert!(cn.starts_with("https://bambulab.cn/sign-in?from=studio"));
    }

    #[test]
    fn callback_query_ticket_and_redirect() {
        let cb = parse_oauth_callback_target(
            "/?ticket=Ab12Cd&redirect_url=https%3A%2F%2Fbambulab.com%2Fen%2F",
        )
        .unwrap();
        assert_eq!(cb.ticket, "Ab12Cd");
        assert_eq!(cb.redirect_url, "https://bambulab.com/en/");
    }

    #[test]
    fn callback_accepts_oauth_code_alias() {
        let cb = parse_oauth_callback_target("/callback?code=Zz9").unwrap();
        assert_eq!(cb.ticket, "Zz9");
    }

    #[test]
    fn callback_ignores_favicon() {
        assert!(parse_oauth_callback_target("/favicon.ico").is_none());
    }

    #[test]
    fn local_listener_receives_ticket() {
        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        let port = listener.local_addr().unwrap().port();
        let worker = thread::spawn(move || {
            thread::sleep(Duration::from_millis(80));
            let mut stream = TcpStream::connect(("127.0.0.1", port)).unwrap();
            stream
                .write_all(
                    b"GET /?ticket=TkT1&redirect_url=https%3A%2F%2Fbambulab.com%2F HTTP/1.1\r\nHost: localhost\r\n\r\n",
                )
                .unwrap();
            let mut buf = [0u8; 512];
            let n = stream.read(&mut buf).unwrap_or(0);
            String::from_utf8_lossy(&buf[..n]).into_owned()
        });
        let cb = accept_oauth_callback_from(&listener, None, Duration::from_secs(2)).unwrap();
        let resp = worker.join().unwrap();
        assert_eq!(cb.ticket, "TkT1");
        assert_eq!(cb.redirect_url, "https://bambulab.com/");
        assert!(resp.starts_with("HTTP/1.1 200"), "{resp}");
        assert!(!resp.contains("Location:"), "{resp}");
        assert!(!resp.contains("bambustudioopen"), "{resp}");
    }

    #[test]
    fn ticket_path_encodes() {
        assert_eq!(
            crate::ticket_path("Ab/1"),
            "/v1/user-service/user/ticket/Ab%2F1"
        );
    }
}
