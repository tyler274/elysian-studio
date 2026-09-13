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

/// One HMS `{attr,code}` pair from MQTT `print.hms` (C++ `DevHMSItem`).
#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
pub struct HmsCode {
    pub attr: u32,
    pub code: u32,
}

impl HmsCode {
    /// C++ `DevHMSItem::get_long_error_code` (`%02X%02X%02X%02X00%02X%04X`).
    pub fn long_error_code(self) -> String {
        let module_id = (self.attr >> 24) & 0xFF;
        let module_num = (self.attr >> 16) & 0xFF;
        let part_id = (self.attr >> 8) & 0xFF;
        let reserved = self.attr & 0xFF;
        let msg_level = (self.code >> 16) & 0xFF;
        let msg_code = self.code & 0xFFFF;
        format!("{module_id:02X}{module_num:02X}{part_id:02X}{reserved:02X}00{msg_level:02X}{msg_code:04X}")
    }
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
    /// `print.mc_percent` (0–100).
    pub mc_percent: u8,
    /// `print.layer_num`.
    pub layer_num: u32,
    /// `print.total_layer_num`.
    pub total_layer_num: u32,
    /// `print.mc_remaining_time` (minutes).
    pub mc_remaining_time_min: u32,
    /// `print.gcode_state` (`IDLE`, `RUNNING`, `PAUSE`, …).
    pub gcode_state: String,
    /// `print.wifi_signal` (e.g. `-44dBm`).
    pub wifi_signal: String,
    pub hms: Vec<HmsCode>,
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
            mc_percent: 0,
            layer_num: 0,
            total_layer_num: 0,
            mc_remaining_time_min: 0,
            gcode_state: String::new(),
            wifi_signal: String::new(),
            hms: Vec::new(),
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AmsTray {
    pub id: u8,
    pub filament_type: String,
    pub color: String,
    pub remain: Option<u8>,
    pub humidity: Option<u8>,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct AmsState {
    pub slot_count: u8,
    pub active_slot: Option<u8>,
    pub trays: Vec<AmsTray>,
    pub mapping: Vec<i32>,
    /// Unit humidity (firmware enum / percent; printer-dependent).
    pub humidity: Option<u8>,
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
    fn pause(&self) -> impl Future<Output = Result<(), DeviceError>> + Send;
    fn resume(&self) -> impl Future<Output = Result<(), DeviceError>> + Send;
    fn stop(&self) -> impl Future<Output = Result<(), DeviceError>> + Send;
    fn set_print_speed(&self, level: u8) -> impl Future<Output = Result<(), DeviceError>> + Send;
    fn set_chamber_light(&self, on: bool) -> impl Future<Output = Result<(), DeviceError>> + Send;
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn hms_long_error_code_matches_studio() {
        // attr 0x0700_0100, code 0x0001_0001 → 07 00 01 00 00 01 0001
        let hms = HmsCode {
            attr: 0x0700_0100,
            code: 0x0001_0001,
        };
        assert_eq!(hms.long_error_code(), "0700010000010001");
    }
}
