//! LAN MQTT topics and `push_status` parsing (OpenBambuAPI / open-bamboo-networking).

use std::sync::atomic::{AtomicU64, Ordering};

use bambu_device::{AmsState, MachineState};
use serde_json::Value;

pub const LAN_MQTT_PORT: u16 = 8883;
pub const LAN_MQTT_USER: &str = "bblp";

/// Stock plugin seeds `project_file` in 20000–29999; reusing 20001 across process
/// restarts can yield `err_code` 84033544.
pub fn next_sequence_id() -> u64 {
    static SEQ: AtomicU64 = AtomicU64::new(0);
    let mut n = SEQ.fetch_add(1, Ordering::Relaxed);
    if n == 0 {
        let seed = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map(|d| d.as_nanos() as u64)
            .unwrap_or(1);
        SEQ.store(seed, Ordering::Relaxed);
        n = SEQ.fetch_add(1, Ordering::Relaxed);
    }
    20_000 + (n % 10_000)
}

pub fn report_topic(serial: &str) -> String {
    format!("device/{serial}/report")
}

pub fn request_topic(serial: &str) -> String {
    format!("device/{serial}/request")
}

/// Build a `gcode_line` request (unsigned). Sign with [`crate::signing::maybe_sign`].
pub fn gcode_line(sequence_id: u64, gcode: &str) -> String {
    serde_json::json!({
        "print": {
            "sequence_id": sequence_id.to_string(),
            "command": "gcode_line",
            "param": gcode,
        }
    })
    .to_string()
}

pub fn pushall(sequence_id: u64) -> String {
    serde_json::json!({
        "pushing": {
            "sequence_id": sequence_id.to_string(),
            "command": "pushall",
            "version": 1,
            "push_target": 1
        }
    })
    .to_string()
}

/// LAN `project_file` after an FTPS upload. Developer Mode accepts cleartext `url`;
/// non-DM firmware wants `url_enc` (device-cert RSA) which is not wired yet.
pub fn project_file(sequence_id: u64, filename: &str, subtask_name: &str, plate: u32) -> String {
    serde_json::json!({
        "print": {
            "sequence_id": sequence_id.to_string(),
            "command": "project_file",
            "param": format!("Metadata/plate_{plate}.gcode"),
            "project_id": "0",
            "profile_id": "0",
            "task_id": "0",
            "subtask_id": "0",
            "subtask_name": subtask_name,
            "file": filename,
            "url": format!("ftp://{filename}"),
            "md5": "from_sd_card",
            "bed_type": "auto",
            "bed_leveling": false,
            "flow_cali": false,
            "vibration_cali": false,
            "layer_inspect": false,
            "timelapse": false,
            "use_ams": false,
            "ams_mapping": [],
            "auto_bed_leveling": 0,
            "cfg": "0",
            "extrude_cali_flag": 0,
            "nozzle_offset_cali": 2
        }
    })
    .to_string()
}

pub fn parse_push_status(payload: &str) -> Option<MachineState> {
    let v: Value = serde_json::from_str(payload).ok()?;
    let print = v.get("print").or(Some(&v))?;
    let command = print.get("command").and_then(Value::as_str);
    if command.is_some() && command != Some("push_status") {
        return None;
    }
    Some(MachineState {
        serial: print
            .get("dev_id")
            .or_else(|| v.get("dev_id"))
            .and_then(Value::as_str)
            .unwrap_or("")
            .to_string(),
        name: print
            .get("subtask_name")
            .and_then(Value::as_str)
            .unwrap_or("")
            .to_string(),
        online: true,
        nozzle_temp_c: number(print, "nozzle_temper"),
        bed_temp_c: number(print, "bed_temper"),
    })
}

pub fn parse_ams(payload: &str) -> Option<AmsState> {
    let v: Value = serde_json::from_str(payload).ok()?;
    let print = v.get("print")?;
    let ams = print.get("ams")?;
    let slots = ams.get("ams").and_then(Value::as_array)?;
    let active = ams
        .get("tray_now")
        .and_then(Value::as_str)
        .and_then(|s| s.parse().ok());
    Some(AmsState {
        slot_count: slots.len() as u8,
        active_slot: active,
    })
}

fn number(v: &Value, key: &str) -> f32 {
    v.get(key)
        .and_then(|n| {
            n.as_f64()
                .or_else(|| n.as_str().and_then(|s| s.parse().ok()))
        })
        .unwrap_or(0.0) as f32
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_push_status() {
        let json = r#"{
            "print": {
                "command": "push_status",
                "nozzle_temper": 215.25,
                "bed_temper": 60,
                "subtask_name": "cube"
            }
        }"#;
        let st = parse_push_status(json).unwrap();
        assert!((st.nozzle_temp_c - 215.25).abs() < 0.01);
        assert!((st.bed_temp_c - 60.0).abs() < 0.01);
        assert_eq!(st.name, "cube");
    }

    #[test]
    fn project_file_has_lan_url() {
        let json = project_file(20042, "cube.gcode.3mf", "cube", 1);
        let v: Value = serde_json::from_str(&json).unwrap();
        assert_eq!(v["print"]["command"], "project_file");
        assert_eq!(v["print"]["url"], "ftp://cube.gcode.3mf");
        assert_eq!(v["print"]["param"], "Metadata/plate_1.gcode");
        assert_eq!(v["print"]["md5"], "from_sd_card");
        assert_eq!(v["print"]["sequence_id"], "20042");
    }
}
