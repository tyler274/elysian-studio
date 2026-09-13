//! LAN MQTT topics and `push_status` parsing (OpenBambuAPI / open-bamboo-networking).

use std::sync::atomic::{AtomicU64, Ordering};

use bambu_device::{AmsState, AmsTray, HmsCode, MachineState};
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

fn print_command(sequence_id: u64, command: &str, param: &str) -> String {
    serde_json::json!({
        "print": {
            "sequence_id": sequence_id.to_string(),
            "command": command,
            "param": param,
        }
    })
    .to_string()
}

/// C++ `MachineObject::command_task_pause`.
pub fn pause(sequence_id: u64) -> String {
    print_command(sequence_id, "pause", "")
}

/// C++ `MachineObject::command_task_resume`.
pub fn resume(sequence_id: u64) -> String {
    print_command(sequence_id, "resume", "")
}

/// C++ `MachineObject::command_task_abort`.
pub fn stop(sequence_id: u64) -> String {
    print_command(sequence_id, "stop", "")
}

/// C++ `MachineObject::command_set_printing_speed` (`param` is the level int).
pub fn print_speed(sequence_id: u64, level: u8) -> String {
    print_command(sequence_id, "print_speed", &level.to_string())
}

/// C++ `DevLamp::command_set_chamber_light`.
pub fn chamber_light(sequence_id: u64, on: bool) -> String {
    serde_json::json!({
        "system": {
            "command": "ledctrl",
            "led_node": "chamber_light",
            "sequence_id": sequence_id.to_string(),
            "led_mode": if on { "on" } else { "off" },
            "led_on_time": 500,
            "led_off_time": 500,
            "loop_times": 0,
            "interval_time": 0
        }
    })
    .to_string()
}

/// LAN `project_file` after an FTPS upload. Developer Mode requires cleartext `url`;
/// secured firmware (`fun` bit 29) gets `url_enc` in [`crate::signing::maybe_sign_ex`].
pub fn project_file(sequence_id: u64, filename: &str, subtask_name: &str, plate: u32) -> String {
    project_file_with_ams(sequence_id, filename, subtask_name, plate, &[])
}

pub fn project_file_with_ams(
    sequence_id: u64,
    filename: &str,
    subtask_name: &str,
    plate: u32,
    ams_mapping: &[i32],
) -> String {
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
            "use_ams": !ams_mapping.is_empty(),
            "ams_mapping": ams_mapping,
            "auto_bed_leveling": 0,
            "cfg": "0",
            "extrude_cali_flag": 0,
            "nozzle_offset_cali": 2
        }
    })
    .to_string()
}

/// `print.fun` bit 29 set ⇒ Developer Mode **off** (field encryption required).
pub const FUN_BIT_SECURED: u32 = 29;

pub fn parse_fun(v: &Value) -> u64 {
    if let Some(n) = v.as_u64() {
        return n;
    }
    if let Some(s) = v.as_str() {
        let s = s.trim();
        if let Ok(n) = u64::from_str_radix(s.trim_start_matches("0x").trim_start_matches("0X"), 16)
        {
            return n;
        }
        if let Ok(n) = s.parse::<u64>() {
            return n;
        }
    }
    0
}

pub fn developer_mode_from_fun(fun: u64) -> bool {
    (fun & (1u64 << FUN_BIT_SECURED)) == 0
}

pub fn app_cert_install(sequence_id: u64, app_cert_pem: &str, crl_pem: &str) -> String {
    serde_json::json!({
        "security": {
            "command": "app_cert_install",
            "sequence_id": sequence_id.to_string(),
            "app_cert": app_cert_pem,
            "crl": crl_pem
        }
    })
    .to_string()
}

pub fn parse_printer_cert(payload: &str) -> Option<String> {
    let v: Value = serde_json::from_str(payload).ok()?;
    let sec = v.get("security")?;
    let cert = sec.get("printer_cert")?.as_str()?;
    if cert.contains("BEGIN CERTIFICATE") {
        Some(cert.to_string())
    } else {
        None
    }
}

pub fn parse_push_status(payload: &str) -> Option<MachineState> {
    let v: Value = serde_json::from_str(payload).ok()?;
    let print = v.get("print").or(Some(&v))?;
    let command = print.get("command").and_then(Value::as_str);
    if command.is_some() && command != Some("push_status") {
        return None;
    }
    let fun = print.get("fun").map(parse_fun).unwrap_or(0);
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
        fun,
        developer_mode: developer_mode_from_fun(fun),
        mc_percent: uint(print, "mc_percent") as u8,
        layer_num: uint(print, "layer_num"),
        total_layer_num: uint(print, "total_layer_num"),
        mc_remaining_time_min: uint(print, "mc_remaining_time"),
        gcode_state: print
            .get("gcode_state")
            .and_then(Value::as_str)
            .unwrap_or("")
            .to_string(),
        wifi_signal: print
            .get("wifi_signal")
            .and_then(Value::as_str)
            .unwrap_or("")
            .to_string(),
        hms: parse_hms_items(print.get("hms")),
    })
}

/// C++ `DevHMS::ParseHMSItems` on `print.hms`.
pub fn parse_hms_items(hms: Option<&Value>) -> Vec<HmsCode> {
    let Some(arr) = hms.and_then(Value::as_array) else {
        return Vec::new();
    };
    arr.iter()
        .filter_map(|item| {
            let attr = uint(item, "attr");
            let code = uint(item, "code");
            if attr == 0 && code == 0 && item.get("attr").is_none() {
                return None;
            }
            Some(HmsCode {
                attr,
                code,
            })
        })
        .collect()
}

pub fn parse_ams(payload: &str) -> Option<AmsState> {
    let v: Value = serde_json::from_str(payload).ok()?;
    let print = v.get("print")?;
    let ams = print.get("ams")?;
    let slots = ams.get("ams").and_then(Value::as_array)?;
    let active = ams
        .get("tray_now")
        .and_then(Value::as_str)
        .and_then(|s| s.parse().ok())
        .or_else(|| ams.get("tray_now").and_then(Value::as_u64).map(|n| n as u8));
    let mut trays = Vec::new();
    for unit in slots {
        let Some(tray_list) = unit.get("tray").and_then(Value::as_array) else {
            continue;
        };
        for tray in tray_list {
            let id = tray
                .get("id")
                .and_then(|x| {
                    x.as_str()
                        .and_then(|s| s.parse().ok())
                        .or_else(|| x.as_u64().map(|n| n as u8))
                })
                .unwrap_or(trays.len() as u8);
            trays.push(AmsTray {
                id,
                filament_type: tray
                    .get("tray_type")
                    .and_then(Value::as_str)
                    .unwrap_or("")
                    .to_string(),
                color: tray
                    .get("tray_color")
                    .and_then(Value::as_str)
                    .unwrap_or("")
                    .to_string(),
                remain: tray.get("remain").and_then(|x| {
                    x.as_u64()
                        .map(|n| n as u8)
                        .or_else(|| x.as_str().and_then(|s| s.parse().ok()))
                }),
                humidity: optional_u8(tray, "humidity"),
            });
        }
    }
    let mapping = ams
        .get("ams_mapping")
        .and_then(Value::as_array)
        .map(|arr| {
            arr.iter()
                .filter_map(|x| {
                    x.as_i64()
                        .map(|n| n as i32)
                        .or_else(|| x.as_str().and_then(|s| s.parse().ok()))
                })
                .collect()
        })
        .unwrap_or_default();
    let humidity = slots
        .iter()
        .find_map(|unit| optional_u8(unit, "humidity"))
        .or_else(|| optional_u8(ams, "humidity"));
    Some(AmsState {
        slot_count: trays.len().max(slots.len()) as u8,
        active_slot: active,
        trays,
        mapping,
        humidity,
    })
}

fn optional_u8(v: &Value, key: &str) -> Option<u8> {
    v.get(key).and_then(|n| {
        n.as_u64()
            .map(|n| n as u8)
            .or_else(|| n.as_str().and_then(|s| s.parse().ok()))
    })
}

fn uint(v: &Value, key: &str) -> u32 {
    v.get(key)
        .and_then(|n| {
            n.as_u64()
                .or_else(|| n.as_i64().map(|i| i.max(0) as u64))
                .or_else(|| n.as_str().and_then(|s| s.parse().ok()))
        })
        .unwrap_or(0) as u32
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
        assert_eq!(v["print"]["use_ams"], false);
    }

    #[test]
    fn project_file_with_ams_mapping_enables_use_ams() {
        let json = project_file_with_ams(20042, "cube.gcode.3mf", "cube", 1, &[0, 1, 2]);
        let v: Value = serde_json::from_str(&json).unwrap();
        assert_eq!(v["print"]["use_ams"], true);
        assert_eq!(
            v["print"]["ams_mapping"].as_array().map(|a| a.len()),
            Some(3)
        );
    }

    #[test]
    fn parse_ams_reads_trays() {
        let json = r#"{
            "print": {
                "ams": {
                    "tray_now": "1",
                    "ams": [{
                        "id": "0",
                        "tray": [
                            {"id": "0", "tray_type": "PLA", "tray_color": "FFFFFFFF", "remain": 80},
                            {"id": "1", "tray_type": "PETG", "tray_color": "000000FF", "remain": 10}
                        ]
                    }]
                }
            }
        }"#;
        let ams = parse_ams(json).unwrap();
        assert_eq!(ams.active_slot, Some(1));
        assert_eq!(ams.trays.len(), 2);
        assert_eq!(ams.trays[0].filament_type, "PLA");
        assert_eq!(ams.trays[1].filament_type, "PETG");
    }

    #[test]
    fn fun_bit_29_is_developer_mode() {
        // ClusterM P2S toggle: …193FF9CB7 (DM on) ↔ …1B3FF9CB7 (secured).
        assert!(developer_mode_from_fun(0x193F_F9CB7));
        assert!(!developer_mode_from_fun(0x1B3F_F9CB7));
        assert_eq!(parse_fun(&Value::String("1B3FF9CB7".into())), 0x1B3F_F9CB7);
        let json = r#"{
            "print": {
                "command": "push_status",
                "fun": "1B3FF9CB7",
                "nozzle_temper": 0,
                "bed_temper": 0
            }
        }"#;
        let st = parse_push_status(json).unwrap();
        assert!(!st.developer_mode);
        assert_eq!(st.fun, 0x1B3F_F9CB7);
    }

    #[test]
    fn pause_resume_stop_match_studio() {
        for (builder, cmd) in [
            (pause as fn(u64) -> String, "pause"),
            (resume, "resume"),
            (stop, "stop"),
        ] {
            let v: Value = serde_json::from_str(&builder(20001)).unwrap();
            assert_eq!(v["print"]["command"], cmd);
            assert_eq!(v["print"]["param"], "");
            assert_eq!(v["print"]["sequence_id"], "20001");
        }
        let speed: Value = serde_json::from_str(&print_speed(20002, 2)).unwrap();
        assert_eq!(speed["print"]["command"], "print_speed");
        assert_eq!(speed["print"]["param"], "2");
        let light: Value = serde_json::from_str(&chamber_light(20003, true)).unwrap();
        assert_eq!(light["system"]["command"], "ledctrl");
        assert_eq!(light["system"]["led_node"], "chamber_light");
        assert_eq!(light["system"]["led_mode"], "on");
    }

    #[test]
    fn parses_progress_and_hms() {
        let json = r#"{
            "print": {
                "command": "push_status",
                "nozzle_temper": 0,
                "bed_temper": 0,
                "mc_percent": 42,
                "layer_num": 12,
                "total_layer_num": 80,
                "mc_remaining_time": 35,
                "gcode_state": "RUNNING",
                "wifi_signal": "-44dBm",
                "hms": [{"attr": 117440768, "code": 65537}]
            }
        }"#;
        let st = parse_push_status(json).unwrap();
        assert_eq!(st.mc_percent, 42);
        assert_eq!(st.layer_num, 12);
        assert_eq!(st.total_layer_num, 80);
        assert_eq!(st.mc_remaining_time_min, 35);
        assert_eq!(st.gcode_state, "RUNNING");
        assert_eq!(st.wifi_signal, "-44dBm");
        assert_eq!(st.hms.len(), 1);
        assert_eq!(st.hms[0].long_error_code(), "0700010000010001");
    }

    #[test]
    fn parse_ams_reads_humidity() {
        let json = r#"{
            "print": {
                "ams": {
                    "tray_now": "1",
                    "humidity": "2",
                    "ams": [{
                        "id": "0",
                        "humidity": "3",
                        "tray": [
                            {"id": "0", "tray_type": "PLA", "tray_color": "FFFFFFFF", "remain": 80, "humidity": "4"}
                        ]
                    }]
                }
            }
        }"#;
        let ams = parse_ams(json).unwrap();
        assert_eq!(ams.humidity, Some(3));
        assert_eq!(ams.trays[0].humidity, Some(4));
    }
}
