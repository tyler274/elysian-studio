#![forbid(unsafe_code)]

use std::future::Future;

use serde::{Deserialize, Serialize};
use thiserror::Error;

#[derive(Debug, Error)]
pub enum DeviceError {
    #[error("{0}")]
    Message(String),
    #[error("not implemented")]
    NotImplemented,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct MachineState {
    pub serial: String,
    pub name: String,
    pub online: bool,
    pub nozzle_temp_c: f32,
    pub bed_temp_c: f32,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct AmsState {
    pub slot_count: u8,
    pub active_slot: Option<u8>,
}

#[derive(Debug, Clone)]
pub struct Frame {
    pub width: u32,
    pub height: u32,
    pub rgba: Vec<u8>,
}

#[derive(Debug, Clone)]
pub struct PrintJob {
    pub filename: String,
    pub gcode: String,
}

pub trait PrinterBackend {
    fn status(&self) -> impl Future<Output = Result<MachineState, DeviceError>> + Send;
    fn start_print(
        &self,
        job: PrintJob,
    ) -> impl Future<Output = Result<(), DeviceError>> + Send;
    fn ams(&self) -> impl Future<Output = Result<AmsState, DeviceError>> + Send;
    fn camera_frame(&self) -> impl Future<Output = Result<Frame, DeviceError>> + Send;
}
