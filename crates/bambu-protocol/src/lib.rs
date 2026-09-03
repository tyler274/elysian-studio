#![forbid(unsafe_code)]

mod camera;
mod credentials;
mod extract;
mod ftps;
mod lan_mqtt;
mod mqtt;
mod pack;
mod signing;
mod ssdp;
mod tls;

use std::time::Duration;

use bambu_device::{AmsState, DeviceError, Frame, MachineState, PrintJob, PrinterBackend};
use thiserror::Error;

pub use camera::{auth_packet, jpeg_to_frame, snapshot_jpeg, LAN_CAMERA_PORT};
pub use credentials::{
    candidate_import_dirs, default_config_dir, import_from_known_locations, load_device_cert,
    load_from_dir, save_device_cert, write_to_dir, CredentialError, SlicerCredentials,
};
pub use extract::{
    extract_pems_from_bytes, extract_to_config_dir, find_stock_plugin, ExtractReport,
};
pub use ftps::{stor as ftps_stor, LAN_FTPS_PORT};
pub use mqtt::{
    app_cert_install, gcode_line, next_sequence_id, parse_ams, parse_printer_cert,
    parse_push_status, project_file, pushall, report_topic, request_topic, LAN_MQTT_PORT,
    LAN_MQTT_USER,
};
pub use pack::{pack_gcode_3mf, sanitize_remote_name};
pub use signing::{encrypt_field, maybe_sign, maybe_sign_ex, slicer_cert_id, SigningError};
pub use ssdp::{
    discover, parse_ssdp, printer_from_headers, DiscoveredPrinter, SsdpError, SSDP_PORT,
};
pub use tls::{peek_peer_cn, peek_peer_leaf, TlsError};

#[derive(Debug, Error)]
pub enum ProtocolError {
    #[error(transparent)]
    Device(#[from] DeviceError),
    #[error(transparent)]
    Credential(#[from] CredentialError),
    #[error(transparent)]
    Signing(#[from] SigningError),
    #[error(transparent)]
    Ssdp(#[from] SsdpError),
    #[error(transparent)]
    Tls(#[from] TlsError),
    #[error(transparent)]
    Ftps(#[from] ftps::FtpsError),
    #[error(transparent)]
    Pack(#[from] pack::PackError),
    #[error(transparent)]
    Mqtt(#[from] lan_mqtt::MqttSessionError),
    #[error(transparent)]
    Camera(#[from] camera::CameraError),
}

/// LAN MQTT/FTPS backend (OpenBambuAPI + open-bamboo-networking).
#[derive(Debug, Clone)]
pub struct LanBackend {
    pub host: String,
    pub access_code: String,
    pub serial: String,
    pub credentials: SlicerCredentials,
}

impl LanBackend {
    pub fn new(host: impl Into<String>, access_code: impl Into<String>) -> Self {
        Self {
            host: host.into(),
            access_code: access_code.into(),
            serial: String::new(),
            credentials: SlicerCredentials::default(),
        }
    }

    pub fn with_serial(mut self, serial: impl Into<String>) -> Self {
        self.serial = serial.into();
        self
    }

    pub fn with_credentials(mut self, credentials: SlicerCredentials) -> Self {
        self.credentials = credentials;
        self
    }

    fn map_err(err: impl std::fmt::Display) -> DeviceError {
        DeviceError::Message(err.to_string())
    }

    fn resolved_serial(&self) -> String {
        if !self.serial.is_empty() {
            return self.serial.clone();
        }
        tls::peek_peer_cn(&self.host, LAN_MQTT_PORT).unwrap_or_default()
    }

    fn device_cert_pem(&self) -> Option<String> {
        let serial = self.resolved_serial();
        if !serial.is_empty() {
            if let Ok(Some(pem)) = load_device_cert(default_config_dir(), &serial) {
                return Some(pem);
            }
        }
        let leaf = peek_peer_leaf(&self.host, LAN_MQTT_PORT).ok()?;
        let serial = if serial.is_empty() {
            leaf.cn.clone()
        } else {
            serial
        };
        let _ = save_device_cert(default_config_dir(), &serial, &leaf.pem);
        Some(leaf.pem)
    }

    async fn publish(
        &self,
        payload: &str,
        device_cert: Option<&str>,
        secured: bool,
        wait: Duration,
    ) -> Result<Option<String>, DeviceError> {
        lan_mqtt::publish_signed(lan_mqtt::PublishRequest {
            host: &self.host,
            access_code: &self.access_code,
            serial: &self.serial,
            payload,
            creds: &self.credentials,
            device_cert_pem: device_cert,
            secured,
            wait_report: wait,
        })
        .await
        .map_err(Self::map_err)
    }
}

impl PrinterBackend for LanBackend {
    async fn status(&self) -> Result<MachineState, DeviceError> {
        let (mut state, _) = lan_mqtt::fetch_status(
            &self.host,
            &self.access_code,
            &self.serial,
            Duration::from_secs(8),
        )
        .await
        .map_err(Self::map_err)?;
        if state.serial.is_empty() {
            state.serial = self.resolved_serial();
        }
        let _ = self.device_cert_pem();
        Ok(state)
    }

    async fn start_print(&self, job: PrintJob) -> Result<(), DeviceError> {
        let (state, _) = lan_mqtt::fetch_status(
            &self.host,
            &self.access_code,
            &self.serial,
            Duration::from_secs(8),
        )
        .await
        .map_err(Self::map_err)?;
        let secured = !state.developer_mode;
        if secured && self.credentials.can_install_app_cert() {
            let payload = app_cert_install(
                next_sequence_id(),
                self.credentials.cert_pem.as_deref().unwrap_or(""),
                self.credentials.crl_pem.as_deref().unwrap_or(""),
            );
            let report = self
                .publish(&payload, None, false, Duration::from_secs(8))
                .await?;
            if let Some(body) = report {
                if let Some(pem) = parse_printer_cert(&body) {
                    let serial = self.resolved_serial();
                    if !serial.is_empty() {
                        let _ = save_device_cert(default_config_dir(), &serial, &pem);
                    }
                }
            }
        }
        let device_cert = self.device_cert_pem();
        let stem = sanitize_remote_name(&job.filename);
        let remote = format!("{stem}.gcode.3mf");
        let archive = pack_gcode_3mf(&job.gcode).map_err(Self::map_err)?;
        tracing::info!(
            "FTPS STOR {} -> {}:{} ({} bytes, secured={secured})",
            remote,
            self.host,
            ftps::LAN_FTPS_PORT,
            archive.len()
        );
        ftps::stor(&self.host, &self.access_code, &remote, &archive).map_err(Self::map_err)?;
        let payload = project_file(next_sequence_id(), &remote, &stem, 1);
        let report = self
            .publish(
                &payload,
                device_cert.as_deref(),
                secured,
                Duration::from_secs(8),
            )
            .await?;
        if let Some(body) = report {
            if body.contains("print_error") || body.contains("\"result\":\"fail\"") {
                return Err(DeviceError::Message(format!(
                    "printer rejected print: {body}"
                )));
            }
        }
        Ok(())
    }

    async fn ams(&self) -> Result<AmsState, DeviceError> {
        let (_, ams) = lan_mqtt::fetch_status(
            &self.host,
            &self.access_code,
            &self.serial,
            Duration::from_secs(8),
        )
        .await
        .map_err(Self::map_err)?;
        ams.ok_or(DeviceError::Message(
            "push_status had no AMS block (external spool or older firmware)".into(),
        ))
    }

    async fn camera_frame(&self) -> Result<Frame, DeviceError> {
        camera::snapshot_frame(&self.host, &self.access_code).map_err(Self::map_err)
    }
}

/// Publish a signed or unsigned MQTT `gcode_line`.
pub async fn send_gcode_line(
    backend: &LanBackend,
    line: &str,
) -> Result<Option<String>, ProtocolError> {
    let (state, _) = lan_mqtt::fetch_status(
        &backend.host,
        &backend.access_code,
        &backend.serial,
        Duration::from_secs(8),
    )
    .await?;
    let payload = gcode_line(next_sequence_id(), line);
    let device_cert = backend.device_cert_pem();
    Ok(lan_mqtt::publish_signed(lan_mqtt::PublishRequest {
        host: &backend.host,
        access_code: &backend.access_code,
        serial: &backend.serial,
        payload: &payload,
        creds: &backend.credentials,
        device_cert_pem: device_cert.as_deref(),
        secured: !state.developer_mode,
        wait_report: Duration::from_secs(5),
    })
    .await?)
}

pub async fn install_app_cert(backend: &LanBackend) -> Result<Option<String>, ProtocolError> {
    if !backend.credentials.can_install_app_cert() {
        return Err(ProtocolError::Credential(CredentialError::Message(
            "need slicer_cert.pem and slicer_crl.pem for app_cert_install".into(),
        )));
    }
    let payload = app_cert_install(
        next_sequence_id(),
        backend.credentials.cert_pem.as_deref().unwrap_or(""),
        backend.credentials.crl_pem.as_deref().unwrap_or(""),
    );
    let report = lan_mqtt::publish_signed(lan_mqtt::PublishRequest {
        host: &backend.host,
        access_code: &backend.access_code,
        serial: &backend.serial,
        payload: &payload,
        creds: &backend.credentials,
        device_cert_pem: None,
        secured: false,
        wait_report: Duration::from_secs(8),
    })
    .await?;
    if let Some(body) = &report {
        if let Some(pem) = parse_printer_cert(body) {
            let serial = backend.resolved_serial();
            if !serial.is_empty() {
                save_device_cert(default_config_dir(), &serial, &pem)?;
            }
        }
    }
    Ok(report)
}
