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

/// C++ `MachineObject::command_set_bed` (typed MQTT, not `gcode_line` M140).
pub fn set_bed_temp(sequence_id: u64, temp_c: u16) -> String {
    serde_json::json!({
        "print": {
            "sequence_id": sequence_id.to_string(),
            "command": "set_bed_temp",
            "temp": temp_c,
        }
    })
    .to_string()
}

/// C++ `MachineObject::command_set_nozzle_new`.
pub fn set_nozzle_temp(sequence_id: u64, temp_c: u16) -> String {
    serde_json::json!({
        "print": {
            "sequence_id": sequence_id.to_string(),
            "command": "set_nozzle_temp",
            "extruder_index": 0,
            "target_temp": temp_c,
        }
    })
    .to_string()
}

/// C++ `DevFan::command_control_fan_new` (`fan_index` + `speed` 0–255).
pub fn set_fan(sequence_id: u64, fan_index: u8, speed: u8) -> String {
    serde_json::json!({
        "print": {
            "sequence_id": sequence_id.to_string(),
            "command": "set_fan",
            "fan_index": fan_index,
            "speed": speed,
        }
    })
    .to_string()
}

/// C++ `MachineObject::command_ams_change_filament` (`load` vs unload `target`/`slot_id` 255).
pub fn ams_change_filament(
    sequence_id: u64,
    load: bool,
    ams_id: u8,
    slot_id: u8,
    old_temp: u16,
    new_temp: u16,
) -> String {
    let tray_id = if ams_id < 16 {
        u16::from(ams_id) * 4 + u16::from(slot_id)
    } else {
        0
    };
    let (target, slot) = if load {
        let target = if tray_id == 0 {
            u16::from(ams_id)
        } else {
            tray_id
        };
        (target, u16::from(slot_id))
    } else {
        (255, 255)
    };
    serde_json::json!({
        "print": {
            "command": "ams_change_filament",
            "sequence_id": sequence_id.to_string(),
            "curr_temp": old_temp,
            "tar_temp": new_temp,
            "ams_id": ams_id,
            "target": target,
            "slot_id": slot,
        }
    })
    .to_string()
}

/// C++ `MachineObject::command_hms_resume`.
pub fn hms_resume(sequence_id: u64, err: &str, job_id: &str) -> String {
    hms_dismiss(sequence_id, "resume", err, job_id)
}

/// C++ `MachineObject::command_hms_ignore`.
pub fn hms_ignore(sequence_id: u64, err: &str, job_id: &str) -> String {
    hms_dismiss(sequence_id, "ignore", err, job_id)
}

/// C++ `MachineObject::command_hms_stop`.
pub fn hms_stop(sequence_id: u64, err: &str, job_id: &str) -> String {
    hms_dismiss(sequence_id, "stop", err, job_id)
}

fn hms_dismiss(sequence_id: u64, command: &str, err: &str, job_id: &str) -> String {
    serde_json::json!({
        "print": {
            "command": command,
            "err": err,
            "param": "reserve",
            "job_id": job_id,
            "sequence_id": sequence_id.to_string(),
        }
    })
    .to_string()
}

/// C++ `MachineObject::command_task_partskip`.
pub fn skip_objects(sequence_id: u64, obj_list: &[u32]) -> String {
    serde_json::json!({
        "print": {
            "command": "skip_objects",
            "obj_list": obj_list,
            "sequence_id": sequence_id.to_string(),
        }
    })
    .to_string()
}

/// Calibration flags on MQTT `project_file`. Defaults are all false (cube-safe LAN).
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct ProjectFileOpts {
    pub bed_leveling: bool,
    pub flow_cali: bool,
    pub vibration_cali: bool,
    pub layer_inspect: bool,
    pub timelapse: bool,
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
    project_file_with_ams_opts(
        sequence_id,
        filename,
        subtask_name,
        plate,
        ams_mapping,
        ProjectFileOpts::default(),
    )
}

pub fn project_file_with_ams_opts(
    sequence_id: u64,
    filename: &str,
    subtask_name: &str,
    plate: u32,
    ams_mapping: &[i32],
    opts: ProjectFileOpts,
) -> String {
    project_file_body(
        sequence_id,
        filename,
        subtask_name,
        plate,
        &format!("ftp://{filename}"),
        "from_sd_card",
        ams_mapping,
        opts,
    )
}

/// Cloud `project_file` after an HTTPS upload (`url` is `https://…`, not `ftp://`).
pub fn project_file_cloud(
    sequence_id: u64,
    filename: &str,
    subtask_name: &str,
    plate: u32,
    url: &str,
    md5: &str,
    ams_mapping: &[i32],
) -> String {
    project_file_cloud_opts(
        sequence_id,
        filename,
        subtask_name,
        plate,
        url,
        md5,
        ams_mapping,
        ProjectFileOpts::default(),
    )
}

#[allow(clippy::too_many_arguments)]
pub fn project_file_cloud_opts(
    sequence_id: u64,
    filename: &str,
    subtask_name: &str,
    plate: u32,
    url: &str,
    md5: &str,
    ams_mapping: &[i32],
    opts: ProjectFileOpts,
) -> String {
    project_file_body(
        sequence_id,
        filename,
        subtask_name,
        plate,
        url,
        md5,
        ams_mapping,
        opts,
    )
}

#[allow(clippy::too_many_arguments)]
fn project_file_body(
    sequence_id: u64,
    filename: &str,
    subtask_name: &str,
    plate: u32,
    url: &str,
    md5: &str,
    ams_mapping: &[i32],
    opts: ProjectFileOpts,
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
            "url": url,
            "md5": md5,
            "bed_type": "auto",
            "bed_leveling": opts.bed_leveling,
            "flow_cali": opts.flow_cali,
            "vibration_cali": opts.vibration_cali,
            "layer_inspect": opts.layer_inspect,
            "timelapse": opts.timelapse,
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
        serial: textish(print, "dev_id")
            .or_else(|| textish(&v, "dev_id"))
            .unwrap_or_default(),
        name: textish(print, "subtask_name").unwrap_or_default(),
        online: true,
        nozzle_temp_c: number(print, "nozzle_temper"),
        bed_temp_c: number(print, "bed_temper"),
        nozzle_target_c: number(print, "nozzle_target_temper"),
        bed_target_c: number(print, "bed_target_temper"),
        chamber_temp_c: number(print, "chamber_temper"),
        cooling_fan: uint(print, "cooling_fan_speed") as u8,
        aux_fan: uint(print, "big_fan1_speed") as u8,
        chamber_fan: uint(print, "big_fan2_speed") as u8,
        heatbreak_fan: uint(print, "heatbreak_fan_speed") as u8,
        spd_lvl: uint(print, "spd_lvl") as u8,
        spd_mag: uint(print, "spd_mag") as u16,
        print_error: uint(print, "print_error"),
        mc_print_stage: textish(print, "mc_print_stage").unwrap_or_default(),
        gcode_file: textish(print, "gcode_file").unwrap_or_default(),
        job_id: textish(print, "job_id").unwrap_or_default(),
        chamber_light_on: chamber_light_from_report(print.get("lights_report")),
        fun,
        developer_mode: developer_mode_from_fun(fun),
        mc_percent: uint(print, "mc_percent") as u8,
        layer_num: uint(print, "layer_num"),
        total_layer_num: uint(print, "total_layer_num"),
        mc_remaining_time_min: uint(print, "mc_remaining_time"),
        gcode_state: textish(print, "gcode_state").unwrap_or_default(),
        wifi_signal: textish(print, "wifi_signal").unwrap_or_default(),
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
            Some(HmsCode { attr, code })
        })
        .collect()
}

pub fn parse_ams(payload: &str) -> Option<AmsState> {
    let v: Value = serde_json::from_str(payload).ok()?;
    let print = v.get("print")?;
    if print.get("ams").is_none() && print.get("vt_tray").is_none() {
        return None;
    }
    let ams = print.get("ams");
    let empty: Vec<Value> = Vec::new();
    let slots = ams
        .and_then(|a| a.get("ams"))
        .and_then(Value::as_array)
        .unwrap_or(&empty);
    let active = ams.and_then(|a| {
        a.get("tray_now")
            .and_then(Value::as_str)
            .and_then(|s| s.parse().ok())
            .or_else(|| a.get("tray_now").and_then(Value::as_u64).map(|n| n as u8))
    });
    let mut trays = Vec::new();
    let mut unit_temp = None;
    for unit in slots {
        if unit_temp.is_none() {
            unit_temp = optional_f32(unit, "temp");
        }
        let ams_id = unit.get("id").and_then(as_u8).unwrap_or(0);
        let Some(tray_list) = unit.get("tray").and_then(Value::as_array) else {
            continue;
        };
        for tray in tray_list {
            trays.push(parse_tray(tray, trays.len() as u8, ams_id));
        }
    }
    let mapping = ams
        .and_then(|a| a.get("ams_mapping"))
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
        .or_else(|| ams.and_then(|a| optional_u8(a, "humidity")));
    let vt_tray = print.get("vt_tray").map(|tray| parse_tray(tray, 254, 254));
    Some(AmsState {
        slot_count: trays.len().max(slots.len()) as u8,
        active_slot: active,
        trays,
        mapping,
        humidity,
        unit_temp,
        vt_tray,
    })
}

fn parse_tray(tray: &Value, fallback_id: u8, ams_id: u8) -> AmsTray {
    let id = tray.get("id").and_then(as_u8).unwrap_or(fallback_id);
    AmsTray {
        id,
        ams_id,
        filament_type: textish(tray, "tray_type").unwrap_or_default(),
        color: textish(tray, "tray_color").unwrap_or_default(),
        remain: optional_u8(tray, "remain"),
        humidity: optional_u8(tray, "humidity"),
        tray_info_idx: textish(tray, "tray_info_idx").unwrap_or_default(),
        temp: optional_f32(tray, "temp"),
    }
}

fn chamber_light_from_report(lights: Option<&Value>) -> bool {
    let Some(arr) = lights.and_then(Value::as_array) else {
        return false;
    };
    arr.iter().any(|item| {
        item.get("node").and_then(Value::as_str) == Some("chamber_light")
            && item
                .get("mode")
                .and_then(Value::as_str)
                .is_some_and(|m| m.eq_ignore_ascii_case("on"))
    })
}

fn as_u8(v: &Value) -> Option<u8> {
    v.as_u64()
        .map(|n| n as u8)
        .or_else(|| v.as_str().and_then(|s| s.parse().ok()))
}

fn textish(v: &Value, key: &str) -> Option<String> {
    v.get(key).and_then(|n| {
        n.as_str()
            .map(str::to_string)
            .or_else(|| n.as_i64().map(|i| i.to_string()))
            .or_else(|| n.as_u64().map(|i| i.to_string()))
    })
}

fn optional_f32(v: &Value, key: &str) -> Option<f32> {
    v.get(key).and_then(|n| {
        n.as_f64()
            .map(|n| n as f32)
            .or_else(|| n.as_str().and_then(|s| s.parse().ok()))
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
        assert_eq!(v["print"]["bed_leveling"], false);
        assert_eq!(v["print"]["timelapse"], false);
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
    fn project_file_cloud_uses_https_url() {
        let json = project_file_cloud(
            9,
            "cube.gcode.3mf",
            "cube",
            1,
            "https://cdn.example/cube.gcode.3mf",
            "d41d8cd98f00b204e9800998ecf8427e",
            &[0],
        );
        let v: Value = serde_json::from_str(&json).unwrap();
        assert_eq!(v["print"]["command"], "project_file");
        assert_eq!(v["print"]["url"], "https://cdn.example/cube.gcode.3mf");
        assert!(!v["print"]["url"].as_str().unwrap().starts_with("ftp://"));
        assert_eq!(v["print"]["md5"], "d41d8cd98f00b204e9800998ecf8427e");
        assert_eq!(v["print"]["use_ams"], true);
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

    #[test]
    fn parses_full_push_status_fixture() {
        let json = include_str!("../tests/fixtures/push_status_full.json");
        let st = parse_push_status(json).unwrap();
        assert_eq!(st.serial, "01P00AFAKE00001");
        assert!((st.nozzle_target_c - 220.0).abs() < 0.01);
        assert!((st.bed_target_c - 65.0).abs() < 0.01);
        assert!((st.chamber_temp_c - 35.0).abs() < 0.01);
        assert_eq!(st.cooling_fan, 8);
        assert_eq!(st.aux_fan, 10);
        assert_eq!(st.chamber_fan, 0);
        assert_eq!(st.heatbreak_fan, 15);
        assert_eq!(st.spd_lvl, 2);
        assert_eq!(st.spd_mag, 100);
        assert_eq!(st.print_error, 0);
        assert_eq!(st.mc_print_stage, "2");
        assert_eq!(st.gcode_file, "cube.gcode");
        assert_eq!(st.job_id, "123456");
        assert!(st.chamber_light_on);
        assert_eq!(st.wifi_signal, "-44dBm");
        assert_eq!(st.hms.len(), 1);
        let ams = parse_ams(json).unwrap();
        assert_eq!(ams.trays.len(), 1);
        assert_eq!(ams.trays[0].tray_info_idx, "GFA00");
        assert!((ams.unit_temp.unwrap() - 28.5).abs() < 0.01);
        let vt = ams.vt_tray.expect("vt_tray");
        assert_eq!(vt.id, 254);
        assert_eq!(vt.filament_type, "PLA");
        assert_eq!(vt.tray_info_idx, "GFA00");
    }

    #[test]
    fn set_bed_nozzle_fan_match_studio() {
        let bed: Value = serde_json::from_str(&set_bed_temp(20010, 65)).unwrap();
        assert_eq!(bed["print"]["command"], "set_bed_temp");
        assert_eq!(bed["print"]["temp"], 65);
        let nozzle: Value = serde_json::from_str(&set_nozzle_temp(20011, 220)).unwrap();
        assert_eq!(nozzle["print"]["command"], "set_nozzle_temp");
        assert_eq!(nozzle["print"]["target_temp"], 220);
        assert_eq!(nozzle["print"]["extruder_index"], 0);
        let fan: Value = serde_json::from_str(&set_fan(20012, 1, 128)).unwrap();
        assert_eq!(fan["print"]["command"], "set_fan");
        assert_eq!(fan["print"]["fan_index"], 1);
        assert_eq!(fan["print"]["speed"], 128);
    }

    #[test]
    fn ams_change_filament_load_and_unload() {
        let load: Value =
            serde_json::from_str(&ams_change_filament(1, true, 0, 1, 220, 220)).unwrap();
        assert_eq!(load["print"]["command"], "ams_change_filament");
        assert_eq!(load["print"]["target"], 1);
        assert_eq!(load["print"]["slot_id"], 1);
        assert_eq!(load["print"]["ams_id"], 0);
        let unload: Value =
            serde_json::from_str(&ams_change_filament(2, false, 0, 0, 0, 0)).unwrap();
        assert_eq!(unload["print"]["target"], 255);
        assert_eq!(unload["print"]["slot_id"], 255);
    }

    #[test]
    fn hms_dismiss_and_skip_objects_match_studio() {
        let resume: Value =
            serde_json::from_str(&hms_resume(3, "0700010000010001", "123456")).unwrap();
        assert_eq!(resume["print"]["command"], "resume");
        assert_eq!(resume["print"]["err"], "0700010000010001");
        assert_eq!(resume["print"]["param"], "reserve");
        assert_eq!(resume["print"]["job_id"], "123456");
        let ignore: Value = serde_json::from_str(&hms_ignore(4, "e", "j")).unwrap();
        assert_eq!(ignore["print"]["command"], "ignore");
        let stop: Value = serde_json::from_str(&hms_stop(5, "e", "j")).unwrap();
        assert_eq!(stop["print"]["command"], "stop");
        let skip: Value = serde_json::from_str(&skip_objects(6, &[1, 2])).unwrap();
        assert_eq!(skip["print"]["command"], "skip_objects");
        assert_eq!(
            skip["print"]["obj_list"].as_array().map(|a| a.len()),
            Some(2)
        );
    }

    #[test]
    fn project_file_opts_flags() {
        let opts = ProjectFileOpts {
            bed_leveling: true,
            flow_cali: false,
            vibration_cali: true,
            layer_inspect: false,
            timelapse: true,
        };
        let json = project_file_with_ams_opts(9, "cube.gcode.3mf", "cube", 1, &[], opts);
        let v: Value = serde_json::from_str(&json).unwrap();
        assert_eq!(v["print"]["bed_leveling"], true);
        assert_eq!(v["print"]["flow_cali"], false);
        assert_eq!(v["print"]["vibration_cali"], true);
        assert_eq!(v["print"]["layer_inspect"], false);
        assert_eq!(v["print"]["timelapse"], true);
        assert_eq!(v["print"]["url"], "ftp://cube.gcode.3mf");
        let cloud = project_file_cloud_opts(
            9,
            "cube.gcode.3mf",
            "cube",
            1,
            "https://cdn.example/cube.gcode.3mf",
            "d41d8cd98f00b204e9800998ecf8427e",
            &[],
            opts,
        );
        let c: Value = serde_json::from_str(&cloud).unwrap();
        assert_eq!(c["print"]["timelapse"], true);
        assert!(c["print"]["url"].as_str().unwrap().starts_with("https://"));
    }
}
