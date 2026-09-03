//! LAN TLS for Bambu printers (self-signed leaf; skip CA/hostname).
//!
//! Matches ClusterM/open-bamboo-networking when `printer.cer` is absent:
//! handshake still verifies the TLS signatures, but not the issuer chain.

use std::net::TcpStream;
use std::sync::{Arc, Once};
use std::time::Duration;

use rustls::client::danger::{HandshakeSignatureValid, ServerCertVerified, ServerCertVerifier};
use rustls::crypto::{verify_tls12_signature, verify_tls13_signature, CryptoProvider};
use rustls::pki_types::{CertificateDer, ServerName, UnixTime};
use rustls::{ClientConfig, ClientConnection, DigitallySignedStruct};
use thiserror::Error;
use x509_parser::prelude::*;

#[derive(Debug, Error)]
pub enum TlsError {
    #[error("tls: {0}")]
    Message(String),
    #[error("io: {0}")]
    Io(#[from] std::io::Error),
}

static INSTALL: Once = Once::new();

pub fn install_ring_provider() {
    INSTALL.call_once(|| {
        let _ = rustls::crypto::ring::default_provider().install_default();
    });
}

#[derive(Debug)]
struct SkipVerify(Arc<CryptoProvider>);

impl SkipVerify {
    fn new() -> Arc<Self> {
        Arc::new(Self(Arc::new(rustls::crypto::ring::default_provider())))
    }
}

impl ServerCertVerifier for SkipVerify {
    fn verify_server_cert(
        &self,
        _end_entity: &CertificateDer<'_>,
        _intermediates: &[CertificateDer<'_>],
        _server_name: &ServerName<'_>,
        _ocsp_response: &[u8],
        _now: UnixTime,
    ) -> Result<ServerCertVerified, rustls::Error> {
        Ok(ServerCertVerified::assertion())
    }

    fn verify_tls12_signature(
        &self,
        message: &[u8],
        cert: &CertificateDer<'_>,
        dss: &DigitallySignedStruct,
    ) -> Result<HandshakeSignatureValid, rustls::Error> {
        verify_tls12_signature(
            message,
            cert,
            dss,
            &self.0.signature_verification_algorithms,
        )
    }

    fn verify_tls13_signature(
        &self,
        message: &[u8],
        cert: &CertificateDer<'_>,
        dss: &DigitallySignedStruct,
    ) -> Result<HandshakeSignatureValid, rustls::Error> {
        verify_tls13_signature(
            message,
            cert,
            dss,
            &self.0.signature_verification_algorithms,
        )
    }

    fn supported_verify_schemes(&self) -> Vec<rustls::SignatureScheme> {
        self.0.signature_verification_algorithms.supported_schemes()
    }
}

/// rustls client config that accepts the printer's self-signed certificate.
pub fn lan_client_config() -> Result<Arc<ClientConfig>, TlsError> {
    install_ring_provider();
    let provider = rustls::crypto::ring::default_provider();
    let mut config = ClientConfig::builder_with_provider(provider.into())
        .with_safe_default_protocol_versions()
        .map_err(|err| TlsError::Message(err.to_string()))?
        .dangerous()
        .with_custom_certificate_verifier(SkipVerify::new())
        .with_no_client_auth();
    config.resumption = rustls::client::Resumption::in_memory_sessions(128);
    Ok(Arc::new(config))
}

pub fn server_name(host: &str) -> Result<ServerName<'static>, TlsError> {
    ServerName::try_from(host.to_string())
        .map_err(|err| TlsError::Message(format!("server name {host}: {err}")))
}

/// Handshake MQTT/FTPS and return the leaf certificate CN (usually the serial).
pub fn peek_peer_cn(host: &str, port: u16) -> Result<String, TlsError> {
    let config = lan_client_config()?;
    let name = server_name(host)?;
    let mut tcp = TcpStream::connect((host, port))?;
    tcp.set_read_timeout(Some(Duration::from_secs(8)))?;
    tcp.set_write_timeout(Some(Duration::from_secs(8)))?;
    let mut conn =
        ClientConnection::new(config, name).map_err(|err| TlsError::Message(err.to_string()))?;
    while conn.is_handshaking() {
        conn.complete_io(&mut tcp)
            .map_err(|err| TlsError::Message(err.to_string()))?;
    }
    let der = conn
        .peer_certificates()
        .and_then(|certs| certs.first())
        .ok_or_else(|| TlsError::Message("no peer certificate".into()))?;
    cert_common_name(der.as_ref())
}

pub fn cert_common_name(der: &[u8]) -> Result<String, TlsError> {
    let (_, cert) = parse_x509_certificate(der)
        .map_err(|err| TlsError::Message(format!("peer x509: {err}")))?;
    for rdn in cert.subject().iter_rdn() {
        for attr in rdn.iter() {
            if attr.attr_type().to_id_string() == "2.5.4.3" {
                if let Ok(cn) = attr.as_str() {
                    if !cn.is_empty() {
                        return Ok(cn.to_string());
                    }
                }
            }
        }
    }
    Err(TlsError::Message("peer certificate has no CN".into()))
}
