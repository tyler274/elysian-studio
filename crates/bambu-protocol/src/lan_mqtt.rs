//! Live LAN MQTT session (TLS 8883, user `bblp`, password = access code).

use std::sync::Arc;
use std::time::{Duration, Instant};

use rumqttc::{AsyncClient, Event, Incoming, MqttOptions, QoS, Transport};
use rustls::ClientConfig;
use thiserror::Error;

use crate::credentials::SlicerCredentials;
use crate::mqtt::{
    parse_ams, parse_push_status, pushall, report_topic, request_topic, LAN_MQTT_PORT,
    LAN_MQTT_USER,
};
use crate::signing::maybe_sign;
use crate::tls::{lan_client_config, peek_peer_cn, TlsError};
use bambu_device::{AmsState, MachineState};

#[derive(Debug, Error)]
pub enum MqttSessionError {
    #[error("mqtt: {0}")]
    Message(String),
    #[error(transparent)]
    Tls(#[from] TlsError),
    #[error(transparent)]
    Signing(#[from] crate::signing::SigningError),
}

fn mqtt_options(
    host: &str,
    access_code: &str,
    config: Arc<ClientConfig>,
) -> Result<MqttOptions, MqttSessionError> {
    if access_code.is_empty() {
        return Err(MqttSessionError::Message(
            "LAN access code is empty (printer settings → LAN)".into(),
        ));
    }
    let client_id = format!("bambu-rs-{}", std::process::id());
    let mut opts = MqttOptions::new(client_id, host, LAN_MQTT_PORT);
    opts.set_credentials(LAN_MQTT_USER, access_code);
    opts.set_keep_alive(Duration::from_secs(60));
    opts.set_clean_session(true);
    opts.set_max_packet_size(512 * 1024, 512 * 1024);
    opts.set_transport(Transport::tls_with_config(
        rumqttc::TlsConfiguration::Rustls(config),
    ));
    Ok(opts)
}

pub fn resolve_serial(host: &str, serial: &str) -> Result<String, MqttSessionError> {
    if !serial.is_empty() {
        return Ok(serial.to_string());
    }
    tracing::info!("LAN serial not set; reading CN from {host}:{LAN_MQTT_PORT}");
    peek_peer_cn(host, LAN_MQTT_PORT).map_err(Into::into)
}

/// Connect, subscribe to `report`, `pushall`, return the first `push_status`.
pub async fn fetch_status(
    host: &str,
    access_code: &str,
    serial: &str,
    timeout: Duration,
) -> Result<(MachineState, Option<AmsState>), MqttSessionError> {
    let serial = resolve_serial(host, serial)?;
    let config = lan_client_config()?;
    let opts = mqtt_options(host, access_code, config)?;
    let (client, mut eventloop) = AsyncClient::new(opts, 32);
    client
        .subscribe(report_topic(&serial), QoS::AtMostOnce)
        .await
        .map_err(|err| MqttSessionError::Message(err.to_string()))?;
    client
        .publish(
            request_topic(&serial),
            QoS::AtMostOnce,
            false,
            pushall(crate::mqtt::next_sequence_id()),
        )
        .await
        .map_err(|err| MqttSessionError::Message(err.to_string()))?;

    let deadline = Instant::now() + timeout;
    let mut machine = None;
    let mut ams = None;
    while Instant::now() < deadline {
        let left = deadline.saturating_duration_since(Instant::now());
        match tokio::time::timeout(left.min(Duration::from_millis(400)), eventloop.poll()).await {
            Ok(Ok(Event::Incoming(Incoming::Publish(p)))) => {
                let payload = String::from_utf8_lossy(&p.payload);
                if machine.is_none() {
                    if let Some(mut st) = parse_push_status(&payload) {
                        if st.serial.is_empty() {
                            st.serial = serial.clone();
                        }
                        st.online = true;
                        machine = Some(st);
                    }
                }
                if ams.is_none() {
                    ams = parse_ams(&payload);
                }
                if machine.is_some() {
                    break;
                }
            }
            Ok(Ok(_)) => {}
            Ok(Err(err)) => return Err(MqttSessionError::Message(err.to_string())),
            Err(_) => continue,
        }
    }
    let _ = client.disconnect().await;
    machine
        .ok_or_else(|| {
            MqttSessionError::Message(format!(
                "no push_status from {host} serial {serial} within {timeout:?}"
            ))
        })
        .map(|st| (st, ams))
}

pub async fn publish_signed(
    host: &str,
    access_code: &str,
    serial: &str,
    payload: &str,
    creds: &SlicerCredentials,
    wait_report: Duration,
) -> Result<Option<String>, MqttSessionError> {
    let serial = resolve_serial(host, serial)?;
    let signed = maybe_sign(payload, creds)?;
    let config = lan_client_config()?;
    let opts = mqtt_options(host, access_code, config)?;
    let (client, mut eventloop) = AsyncClient::new(opts, 32);
    client
        .subscribe(report_topic(&serial), QoS::AtMostOnce)
        .await
        .map_err(|err| MqttSessionError::Message(err.to_string()))?;
    client
        .publish(
            request_topic(&serial),
            QoS::AtMostOnce,
            false,
            signed.as_bytes().to_vec(),
        )
        .await
        .map_err(|err| MqttSessionError::Message(err.to_string()))?;

    let deadline = Instant::now() + wait_report;
    let mut last = None;
    while Instant::now() < deadline {
        let left = deadline.saturating_duration_since(Instant::now());
        match tokio::time::timeout(left.min(Duration::from_millis(400)), eventloop.poll()).await {
            Ok(Ok(Event::Incoming(Incoming::Publish(p)))) => {
                last = Some(String::from_utf8_lossy(&p.payload).into_owned());
                if last.as_deref().is_some_and(|s| {
                    s.contains("\"result\"") || s.contains("print_error") || s.contains("err_code")
                }) {
                    break;
                }
            }
            Ok(Ok(Event::Incoming(Incoming::PubAck(_)))) => {
                if wait_report.is_zero() {
                    break;
                }
            }
            Ok(Ok(_)) => {}
            Ok(Err(err)) => return Err(MqttSessionError::Message(err.to_string())),
            Err(_) => continue,
        }
    }
    let _ = client.disconnect().await;
    Ok(last)
}
