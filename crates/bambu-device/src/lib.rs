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

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct MachineState {
    pub serial: String,
    pub name: String,
    pub online: bool,
    pub nozzle_temp_c: f32,
    pub bed_temp_c: f32,
    /// Raw `print.fun` capability mask from `push_status`.
    pub fun: u64,
    /// Printer Developer Mode is **on** when `fun` bit 29 is clear.
    pub developer_mode: bool,
}

impl Default for MachineState {
    fn default() -> Self {
        Self {
            serial: String::new(),
            name: String::new(),
            online: false,
            nozzle_temp_c: 0.0,
            bed_temp_c: 0.0,
            fun: 0,
            developer_mode: true,
        }
    }
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
    fn start_print(&self, job: PrintJob) -> impl Future<Output = Result<(), DeviceError>> + Send;
    fn ams(&self) -> impl Future<Output = Result<AmsState, DeviceError>> + Send;
    fn camera_frame(&self) -> impl Future<Output = Result<Frame, DeviceError>> + Send;
}
