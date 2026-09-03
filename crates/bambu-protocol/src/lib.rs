#![forbid(unsafe_code)]

mod credentials;
mod extract;
mod mqtt;
mod signing;
mod ssdp;

use bambu_device::{AmsState, DeviceError, Frame, MachineState, PrintJob, PrinterBackend};
use thiserror::Error;

pub use credentials::{
    candidate_import_dirs, default_config_dir, import_from_known_locations, load_from_dir,
    write_to_dir, CredentialError, SlicerCredentials,
};
pub use extract::{extract_pems_from_bytes, extract_to_config_dir, find_stock_plugin, ExtractReport};
pub use mqtt::{
    gcode_line, parse_ams, parse_push_status, report_topic, request_topic, LAN_MQTT_PORT,
    LAN_MQTT_USER,
};
pub use signing::{maybe_sign, slicer_cert_id, SigningError};
pub use ssdp::{discover, parse_ssdp, printer_from_headers, DiscoveredPrinter, SsdpError, SSDP_PORT};

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
}

impl PrinterBackend for LanBackend {
    async fn status(&self) -> Result<MachineState, DeviceError> {
        Err(DeviceError::Message(format!(
            "LAN MQTT session to {}:{} (user {LAN_MQTT_USER}) is not connected yet",
            self.host, LAN_MQTT_PORT
        )))
    }

    async fn start_print(&self, job: PrintJob) -> Result<(), DeviceError> {
        let unsigned = gcode_line(1, &job.gcode);
        let signed = maybe_sign(&unsigned, &self.credentials)
            .map_err(|err| DeviceError::Message(err.to_string()))?;
        let mode = if self.credentials.can_sign() {
            "signed (Option B)"
        } else {
            "unsigned (enable Developer Mode on the printer, or extract slicer_key.pem)"
        };
        Err(DeviceError::Message(format!(
            "FTPS upload of {} is not wired yet; MQTT {mode} payload is {} bytes",
            job.filename,
            signed.len()
        )))
    }

    async fn ams(&self) -> Result<AmsState, DeviceError> {
        Err(DeviceError::NotImplemented)
    }

    async fn camera_frame(&self) -> Result<Frame, DeviceError> {
        Err(DeviceError::NotImplemented)
    }
}
