#![forbid(unsafe_code)]

use std::future::Future;

use bambu_device::{AmsState, DeviceError, Frame, MachineState, PrintJob, PrinterBackend};
use thiserror::Error;

#[derive(Debug, Error)]
pub enum ProtocolError {
    #[error(transparent)]
    Device(#[from] DeviceError),
}

/// LAN MQTT/FTPS backend. Transport is not wired yet; the trait is the
/// extension point for OpenBambuAPI-class printers.
#[derive(Debug, Clone)]
pub struct LanBackend {
    pub host: String,
    pub access_code: String,
}

impl LanBackend {
    pub fn new(host: impl Into<String>, access_code: impl Into<String>) -> Self {
        Self {
            host: host.into(),
            access_code: access_code.into(),
        }
    }
}

impl PrinterBackend for LanBackend {
    fn status(&self) -> impl Future<Output = Result<MachineState, DeviceError>> + Send {
        async { Err(DeviceError::NotImplemented) }
    }

    fn start_print(
        &self,
        _job: PrintJob,
    ) -> impl Future<Output = Result<(), DeviceError>> + Send {
        async { Err(DeviceError::NotImplemented) }
    }

    fn ams(&self) -> impl Future<Output = Result<AmsState, DeviceError>> + Send {
        async { Err(DeviceError::NotImplemented) }
    }

    fn camera_frame(&self) -> impl Future<Output = Result<Frame, DeviceError>> + Send {
        async { Err(DeviceError::NotImplemented) }
    }
}
