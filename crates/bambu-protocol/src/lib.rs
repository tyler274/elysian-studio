#![forbid(unsafe_code)]

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

pub use credentials::{
    candidate_import_dirs, default_config_dir, import_from_known_locations, load_from_dir,
    write_to_dir, CredentialError, SlicerCredentials,
};
pub use extract::{
    extract_pems_from_bytes, extract_to_config_dir, find_stock_plugin, ExtractReport,
};
pub use ftps::{stor as ftps_stor, LAN_FTPS_PORT};
pub use mqtt::{
    gcode_line, next_sequence_id, parse_ams, parse_push_status, project_file, pushall,
    report_topic, request_topic, LAN_MQTT_PORT, LAN_MQTT_USER,
};
pub use pack::{pack_gcode_3mf, sanitize_remote_name};
pub use signing::{maybe_sign, slicer_cert_id, SigningError};
pub use ssdp::{
    discover, parse_ssdp, printer_from_headers, DiscoveredPrinter, SsdpError, SSDP_PORT,
};
pub use tls::{peek_peer_cn, TlsError};

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
}

impl PrinterBackend for LanBackend {
    async fn status(&self) -> Result<MachineState, DeviceError> {
        let (state, _) = lan_mqtt::fetch_status(
            &self.host,
            &self.access_code,
            &self.serial,
            Duration::from_secs(8),
        )
        .await
        .map_err(Self::map_err)?;
        Ok(state)
    }

    async fn start_print(&self, job: PrintJob) -> Result<(), DeviceError> {
        let stem = sanitize_remote_name(&job.filename);
        let remote = format!("{stem}.gcode.3mf");
        let archive = pack_gcode_3mf(&job.gcode).map_err(Self::map_err)?;
        tracing::info!(
            "FTPS STOR {} -> {}:{} ({} bytes)",
            remote,
            self.host,
            ftps::LAN_FTPS_PORT,
            archive.len()
        );
        ftps::stor(&self.host, &self.access_code, &remote, &archive).map_err(Self::map_err)?;
        let payload = project_file(next_sequence_id(), &remote, &stem, 1);
        let report = lan_mqtt::publish_signed(
            &self.host,
            &self.access_code,
            &self.serial,
            &payload,
            &self.credentials,
            Duration::from_secs(8),
        )
        .await
        .map_err(Self::map_err)?;
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
        Err(DeviceError::NotImplemented)
    }
}

/// Publish a signed or unsigned MQTT `gcode_line`.
pub async fn send_gcode_line(
    backend: &LanBackend,
    line: &str,
) -> Result<Option<String>, ProtocolError> {
    let payload = gcode_line(next_sequence_id(), line);
    Ok(lan_mqtt::publish_signed(
        &backend.host,
        &backend.access_code,
        &backend.serial,
        &payload,
        &backend.credentials,
        Duration::from_secs(5),
    )
    .await?)
}
