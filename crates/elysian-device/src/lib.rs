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
    pub nozzle_target_c: f32,
    pub bed_target_c: f32,
    pub chamber_temp_c: f32,
    pub cooling_fan: u8,
    pub aux_fan: u8,
    pub chamber_fan: u8,
    pub heatbreak_fan: u8,
    pub spd_lvl: u8,
    pub spd_mag: u16,
    pub print_error: u32,
    pub mc_print_stage: String,
    pub gcode_file: String,
    pub job_id: String,
    pub chamber_light_on: bool,
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
    /// H2C induction rack (`print.device.holder` + rack nozzles). Empty on P1/A1.
    #[serde(default)]
    pub nozzle_rack: NozzleRackState,
    /// MQTT `info.get_version` module `ota.sw_ver` (Studio `dev_ver` for ttcode).
    #[serde(default)]
    pub ota_version: String,
}

impl Default for MachineState {
    fn default() -> Self {
        Self {
            serial: String::new(),
            name: String::new(),
            online: false,
            nozzle_temp_c: 0.0,
            bed_temp_c: 0.0,
            nozzle_target_c: 0.0,
            bed_target_c: 0.0,
            chamber_temp_c: 0.0,
            cooling_fan: 0,
            aux_fan: 0,
            chamber_fan: 0,
            heatbreak_fan: 0,
            spd_lvl: 0,
            spd_mag: 0,
            print_error: 0,
            mc_print_stage: String::new(),
            gcode_file: String::new(),
            job_id: String::new(),
            chamber_light_on: false,
            fun: 0,
            developer_mode: true,
            mc_percent: 0,
            layer_num: 0,
            total_layer_num: 0,
            mc_remaining_time_min: 0,
            gcode_state: String::new(),
            wifi_signal: String::new(),
            hms: Vec::new(),
            nozzle_rack: NozzleRackState::default(),
            ota_version: String::new(),
        }
    }
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct AmsTray {
    pub id: u8,
    pub ams_id: u8,
    pub filament_type: String,
    pub color: String,
    pub remain: Option<u8>,
    pub humidity: Option<u8>,
    pub tray_info_idx: String,
    pub temp: Option<f32>,
}

/// One physical AMS / AMS 2 Pro / N3S unit (`print.ams.ams[]`).
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct AmsUnit {
    pub id: u8,
    /// Studio `DevAmsType` from `info` bits 0..4 (1 AMS, 2 Lite, 3 AMS 2 Pro, 4 AMS HT).
    #[serde(default)]
    pub ams_type: u8,
    pub humidity: Option<u8>,
    pub humidity_percent: Option<u8>,
    pub temp: Option<f32>,
    /// Remaining drying time in minutes.
    pub dry_time_min: Option<u32>,
    /// Studio `DevAms::DryStatus` from `info` bits 4..8.
    pub dry_status: u8,
    /// Studio `DevAms::DrySubStatus` from `info` bits 22..24.
    #[serde(default)]
    pub dry_sub_status: u8,
}

impl AmsUnit {
    pub fn is_drying(&self) -> bool {
        matches!(self.dry_status, 1 | 2 | 5 | 6) || self.dry_time_min.is_some_and(|t| t > 0)
    }

    pub fn supports_drying(&self) -> bool {
        self.ams_type == 3 || self.ams_type == 4 || self.humidity_percent.is_some()
    }

    /// Studio `DevAms::GetDisplayName` (`AMS(%d)` / `AMS 2 Pro(%d)` / …).
    pub fn display_name(&self) -> String {
        let loc = if self.id > 127 {
            u32::from(self.id) - 127
        } else if (0x10..=0x1f).contains(&self.id) {
            u32::from(self.id) - 15
        } else {
            u32::from(self.id) + 1
        };
        format!("{}({loc})", self.type_label())
    }

    pub fn type_label(&self) -> &'static str {
        match self.ams_type {
            2 => "AMS Lite",
            3 => "AMS 2 Pro",
            4 => "AMS HT",
            _ => "AMS",
        }
    }

    /// Studio `AMSDryCtrWin::update_img_description`.
    pub fn dry_status_label(&self) -> &'static str {
        match self.dry_status {
            0 | 3 | 4 => "Idle",
            1 => "Checking",
            2 if self.dry_sub_status == 1 => "Drying-Heating",
            2 if self.dry_sub_status == 2 => "Drying-Dehumidifying",
            2 | 5 | 6 => "Drying",
            _ if self.is_drying() => "Drying",
            _ => "Idle",
        }
    }
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct AmsState {
    pub slot_count: u8,
    pub active_slot: Option<u8>,
    pub trays: Vec<AmsTray>,
    pub mapping: Vec<i32>,
    /// First unit humidity (legacy single-line UI).
    pub humidity: Option<u8>,
    pub unit_temp: Option<f32>,
    /// External spool (`print.vt_tray`).
    pub vt_tray: Option<AmsTray>,
    #[serde(default)]
    pub units: Vec<AmsUnit>,
}

impl AmsState {
    /// Trays, AMS 2 Pro units, or an external spool were in `push_status`.
    pub fn reports_hardware(&self) -> bool {
        !self.trays.is_empty() || self.vt_tray.is_some() || !self.units.is_empty()
    }
}

#[derive(Debug, Clone, Default, Serialize, Deserialize, PartialEq)]
pub struct NozzleSlot {
    pub id: i32,
    pub diameter: f32,
    pub nozzle_type: String,
    pub color: String,
    pub empty: bool,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize, PartialEq)]
pub struct NozzleRackState {
    pub supported: bool,
    pub status: i32,
    pub position: i32,
    pub cali: i32,
    pub toolhead: Vec<NozzleSlot>,
    pub rack: Vec<NozzleSlot>,
}

impl NozzleRackState {
    pub fn reports_slots(&self) -> bool {
        !self.toolhead.is_empty() || !self.rack.is_empty()
    }

    pub fn status_label(&self) -> &'static str {
        match self.status {
            0 => "Idle",
            1 => "Hotend centre",
            2 => "Toolhead centre",
            3 => "Calibrate",
            4 => "Cut material",
            5 => "Unlock hotend",
            6 => "Lift rack",
            7 => "Place hotend",
            8 => "Pick hotend",
            9 => "Lock hotend",
            _ => "Unknown",
        }
    }

    pub fn position_label(&self) -> &'static str {
        match self.position {
            1 => "Row A raised",
            2 => "Row B raised",
            3 => "Centre",
            _ => "Unknown",
        }
    }
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
    fn set_bed_temp(&self, temp_c: u16) -> impl Future<Output = Result<(), DeviceError>> + Send;
    fn set_nozzle_temp(&self, temp_c: u16) -> impl Future<Output = Result<(), DeviceError>> + Send;
    fn set_fan(
        &self,
        fan_index: u8,
        speed: u8,
    ) -> impl Future<Output = Result<(), DeviceError>> + Send;
    fn ams_load(
        &self,
        ams_id: u8,
        slot_id: u8,
        old_temp: u16,
        new_temp: u16,
    ) -> impl Future<Output = Result<(), DeviceError>> + Send;
    fn ams_unload(&self, ams_id: u8) -> impl Future<Output = Result<(), DeviceError>> + Send;
    fn ams_drying(
        &self,
        ams_id: u8,
        filament: &str,
        temp_c: u16,
        duration_h: u16,
        rotate_tray: bool,
        cooling_temp: u16,
    ) -> impl Future<Output = Result<(), DeviceError>> + Send;
    fn ams_drying_stop(&self, ams_id: u8) -> impl Future<Output = Result<(), DeviceError>> + Send;
    fn nozzle_holder_ctrl(
        &self,
        action: u8,
    ) -> impl Future<Output = Result<(), DeviceError>> + Send;
    fn holder_nozzle_refresh(
        &self,
        id: u32,
    ) -> impl Future<Output = Result<(), DeviceError>> + Send;
    fn nozzle_info_confirm(&self, id: u32) -> impl Future<Output = Result<(), DeviceError>> + Send;
    fn hms_resume(
        &self,
        err: &str,
        job_id: &str,
    ) -> impl Future<Output = Result<(), DeviceError>> + Send;
    fn hms_ignore(
        &self,
        err: &str,
        job_id: &str,
    ) -> impl Future<Output = Result<(), DeviceError>> + Send;
    fn hms_stop(
        &self,
        err: &str,
        job_id: &str,
    ) -> impl Future<Output = Result<(), DeviceError>> + Send;
    fn skip_objects(&self, ids: &[u32]) -> impl Future<Output = Result<(), DeviceError>> + Send;
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
