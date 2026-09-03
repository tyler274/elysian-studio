//! LAN MQTT topics and `push_status` parsing (OpenBambuAPI / open-bamboo-networking).

use bambu_device::{AmsState, MachineState};
use serde_json::Value;

pub const LAN_MQTT_PORT: u16 = 8883;
pub const LAN_MQTT_USER: &str = "bblp";

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
        .and_then(|n| n.as_f64().or_else(|| n.as_str().and_then(|s| s.parse().ok())))
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
}
