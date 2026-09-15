#![forbid(unsafe_code)]

mod agora;
mod agora_ap;
mod camera;
mod cloud;
mod cloud_api;
mod credentials;
mod extract;
mod extract_appcert;
mod extract_bootstrap;
mod extract_elf;
mod extract_live;
mod extract_unpack;
mod ftps;
mod hms;
mod https;
mod inventory;
mod lan_mqtt;
mod mqtt;
mod oauth;
mod pack;
mod rtsps;
mod signing;
mod spools;
mod ssdp;
mod studio_import;
mod tls;
mod tutk;

use std::time::Duration;

use bambu_device::{AmsState, DeviceError, Frame, MachineState, PrintJob, PrinterBackend};
use thiserror::Error;

pub use agora::{
    agora_ap_hosts, agora_area_code, agora_rdt_hello, app_id_from_token, encryption_config,
    encryption_salt, on_encoded_video_frame, on_encoded_video_image_received, parse_agora_url,
    stream_agora_frames, AgoraJoin, ChannelMediaOptions, EncodedVideoFrameInfo, EncryptionConfig,
    RtcConnection, RtcEngineConfig, RtcSession, AREA_CODE_AS, AREA_CODE_CN, AREA_CODE_EU,
    AREA_CODE_GLOB, AREA_CODE_NA, CLIENT_ROLE_AUDIENCE, CLIENT_ROLE_BROADCASTER,
    ENCRYPTION_AES_128_GCM2, VIDEO_CODEC_H264, VIDEO_FRAME_TYPE_DELTA_FRAME,
};
pub use camera::{
    agora_url, auth_packet, capture_chamber, describe_rtsps, jpeg_payload_len, jpeg_to_frame,
    probe_rtsps, read_jpeg_frame, rtsps_url, snapshot_jpeg, snapshot_rtsps_frame,
    stream_rtsps_frames, tutk_url, ChamberCapture, JpegStream, RtspsSession, LAN_CAMERA_PORT,
    LAN_RTSPS_PORT,
};
pub use cloud::{
    cloud_mqtt_host, cloud_mqtt_user, load_cloud_session, load_cloud_session_default,
    save_cloud_session, store_login_tokens, CloudBackend, CloudSession,
};
pub use cloud_api::{
    api_host, bind_path, camera_serial_from_bind, camera_url_key, cloud_error_is_rate_limited,
    cloud_http_error, http_user_id, iot_user_id, jwt_iot_user_id, jwt_user_id, login_body, md5_hex, parse_bind_devices,
    parse_camera_creds, parse_camera_url_key, parse_login, parse_profile, parse_upload_ticket,
    profile_path, refresh_body, slicer_device_id, slicer_http_headers, ticket_body, ticket_path,
    ttcode_after_post, ttcode_get_path, ttcode_path, ttcode_post_body, ttcode_should_retry_get,
    upload_ticket_body, CameraCreds, CameraProto, CameraUrlKey, CloudApi, CloudApiError,
    CloudDevice, CloudProfile, LoginResult, UploadTicket, SLICER_AGENT_VERSION,
    SLICER_CLIENT_VERSION,
};
pub use credentials::{
    candidate_import_dirs, default_config_dir, import_from_known_locations, load_device_cert,
    load_from_dir, save_device_cert, write_to_dir, CredentialError, SlicerCredentials,
};
pub use extract::{
    extract_keys, extract_pems_from_bytes, extract_to_config_dir, find_all_stock_plugins,
    find_stock_plugin, ExtractKeysOpts, ExtractReport,
};
pub use ftps::{stor as ftps_stor, LAN_FTPS_PORT};
pub use hms::{
    catalog_cache_path, describe_hms, fetch_catalog, load_cached_catalog, lookup_hms_intro,
    refresh_catalog, save_cached_catalog, HMS_HOST,
};
pub use inventory::{
    initial_weight, length_from_weight, load_inventory, remain_percent, remaining_weight,
    save_inventory, spool_tare, weight_from_length, Inventory, InventoryFilament, InventorySpool,
    InventoryVendor,
};
pub use mqtt::{
    ams_change_filament, ams_filament_drying, app_cert_install, auto_stop_ams_dry, chamber_light,
    gcode_line, hms_ignore, hms_resume, hms_stop, holder_nozzle_refresh, next_sequence_id,
    nozzle_holder_ctrl, nozzle_info_confirm, parse_ams, parse_hms_items, parse_nozzle_rack,
    parse_printer_cert, parse_push_status, pause, print_speed, project_file, project_file_cloud,
    project_file_cloud_opts, project_file_with_ams, project_file_with_ams_opts, pushall,
    report_topic, request_topic, resume, set_bed_temp, set_fan, set_nozzle_temp, skip_objects,
    stop, ProjectFileOpts, AMS_DRY_MODE_OFF, AMS_DRY_MODE_ON_TIME, LAN_MQTT_PORT, LAN_MQTT_USER,
};
pub use oauth::{
    login_with_ticket, oauth_callback_url, oauth_login, open_default_browser, persist_login,
    sign_in_url, wait_for_oauth_callback, web_host, OAuthCallback, OAUTH_CALLBACK_PORT,
};
pub use pack::{pack_gcode_3mf, sanitize_remote_name};
pub use signing::{encrypt_field, maybe_sign, maybe_sign_ex, slicer_cert_id, SigningError};
pub use spools::{
    batch_delete_body, filament_v2_ams_sync_path, filament_v2_batch_path, filament_v2_path,
    load_spools, parse_cloud_filaments, save_spools, spool_from_cloud_json, spool_to_cloud_json,
    spools_path, FilamentSpool,
};
pub use ssdp::{
    discover, parse_ssdp, printer_from_headers, DiscoveredPrinter, SsdpError, SSDP_PORT,
};
pub use studio_import::{
    default_studio_data_dir, import_studio, load_lan_codes, parse_studio_conf, save_lan_codes,
    StudioImport, StudioPrinter,
};
pub use tls::{peek_peer_cn, peek_peer_leaf, TlsError};
pub use tutk::{
    for_each_jpeg as for_each_tutk_jpeg, iotc_masters, is_avc_sample, jpeg_from_av_sample,
    stream_ttcode_frames, stream_ttcode_jpegs, tutk_region, AvFrameInfo, IotcPeer, TutkRegion,
    CODEC_H264, CODEC_HEVC, CODEC_MJPEG,
};

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
    #[error(transparent)]
    Tls(#[from] TlsError),
    #[error(transparent)]
    Ftps(#[from] ftps::FtpsError),
    #[error(transparent)]
    Pack(#[from] pack::PackError),
    #[error(transparent)]
    Mqtt(#[from] lan_mqtt::MqttSessionError),
    #[error(transparent)]
    Camera(#[from] camera::CameraError),
    #[error(transparent)]
    Hms(#[from] hms::HmsError),
    #[error(transparent)]
    Https(#[from] https::HttpsError),
    #[error(transparent)]
    CloudApi(#[from] cloud_api::CloudApiError),
}

/// LAN MQTT/FTPS backend (OpenBambuAPI + open-bamboo-networking).
#[derive(Debug, Clone)]
pub struct LanBackend {
    pub host: String,
    pub access_code: String,
    pub serial: String,
    pub credentials: SlicerCredentials,
    pub ams_mapping: Vec<i32>,
    pub project_opts: ProjectFileOpts,
}

impl LanBackend {
    pub fn new(host: impl Into<String>, access_code: impl Into<String>) -> Self {
        Self {
            host: host.into(),
            access_code: access_code.into(),
            serial: String::new(),
            credentials: SlicerCredentials::default(),
            ams_mapping: Vec::new(),
            project_opts: ProjectFileOpts::default(),
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

    pub fn with_ams_mapping(mut self, mapping: Vec<i32>) -> Self {
        self.ams_mapping = mapping;
        self
    }

    pub fn with_project_opts(mut self, opts: ProjectFileOpts) -> Self {
        self.project_opts = opts;
        self
    }

    fn map_err(err: impl std::fmt::Display) -> DeviceError {
        DeviceError::Message(err.to_string())
    }

    fn resolved_serial(&self) -> String {
        if !self.serial.is_empty() {
            return self.serial.clone();
        }
        tls::peek_peer_cn(&self.host, LAN_MQTT_PORT).unwrap_or_default()
    }

    fn device_cert_pem(&self) -> Option<String> {
        let serial = self.resolved_serial();
        if !serial.is_empty() {
            if let Ok(Some(pem)) = load_device_cert(default_config_dir(), &serial) {
                return Some(pem);
            }
        }
        let leaf = peek_peer_leaf(&self.host, LAN_MQTT_PORT).ok()?;
        let serial = if serial.is_empty() {
            leaf.cn.clone()
        } else {
            serial
        };
        let _ = save_device_cert(default_config_dir(), &serial, &leaf.pem);
        Some(leaf.pem)
    }

    async fn publish(
        &self,
        payload: &str,
        device_cert: Option<&str>,
        secured: bool,
        wait: Duration,
    ) -> Result<Option<String>, DeviceError> {
        lan_mqtt::publish_signed(lan_mqtt::PublishRequest {
            host: &self.host,
            access_code: &self.access_code,
            serial: &self.serial,
            payload,
            creds: &self.credentials,
            device_cert_pem: device_cert,
            secured,
            wait_report: wait,
        })
        .await
        .map_err(Self::map_err)
    }

    async fn command(&self, payload: String) -> Result<(), DeviceError> {
        let (state, _) = lan_mqtt::fetch_status(
            &self.host,
            &self.access_code,
            &self.serial,
            Duration::from_secs(8),
        )
        .await
        .map_err(Self::map_err)?;
        let device_cert = self.device_cert_pem();
        let report = self
            .publish(
                &payload,
                device_cert.as_deref(),
                !state.developer_mode,
                Duration::from_secs(5),
            )
            .await?;
        if let Some(body) = report {
            if body.contains("print_error") || body.contains("\"result\":\"fail\"") {
                return Err(DeviceError::Message(format!(
                    "printer rejected command: {body}"
                )));
            }
        }
        Ok(())
    }
}

impl PrinterBackend for LanBackend {
    async fn status(&self) -> Result<MachineState, DeviceError> {
        let (mut state, _) = lan_mqtt::fetch_status(
            &self.host,
            &self.access_code,
            &self.serial,
            Duration::from_secs(8),
        )
        .await
        .map_err(Self::map_err)?;
        if state.serial.is_empty() {
            state.serial = self.resolved_serial();
        }
        let _ = self.device_cert_pem();
        Ok(state)
    }

    async fn start_print(&self, job: PrintJob) -> Result<(), DeviceError> {
        let (state, _) = lan_mqtt::fetch_status(
            &self.host,
            &self.access_code,
            &self.serial,
            Duration::from_secs(8),
        )
        .await
        .map_err(Self::map_err)?;
        let secured = !state.developer_mode;
        if secured && self.credentials.can_install_app_cert() {
            let payload = app_cert_install(
                next_sequence_id(),
                self.credentials.cert_pem.as_deref().unwrap_or(""),
                self.credentials.crl_pem.as_deref().unwrap_or(""),
            );
            let report = self
                .publish(&payload, None, false, Duration::from_secs(8))
                .await?;
            if let Some(body) = report {
                if let Some(pem) = parse_printer_cert(&body) {
                    let serial = self.resolved_serial();
                    if !serial.is_empty() {
                        let _ = save_device_cert(default_config_dir(), &serial, &pem);
                    }
                }
            }
        }
        let device_cert = self.device_cert_pem();
        let stem = sanitize_remote_name(&job.filename);
        let remote = format!("{stem}.gcode.3mf");
        let archive = pack_gcode_3mf(&job.gcode).map_err(Self::map_err)?;
        tracing::info!(
            "FTPS STOR {} -> {}:{} ({} bytes, secured={secured})",
            remote,
            self.host,
            ftps::LAN_FTPS_PORT,
            archive.len()
        );
        ftps::stor(&self.host, &self.access_code, &remote, &archive).map_err(Self::map_err)?;
        let payload = project_file_with_ams_opts(
            next_sequence_id(),
            &remote,
            &stem,
            1,
            &self.ams_mapping,
            self.project_opts,
        );
        let report = self
            .publish(
                &payload,
                device_cert.as_deref(),
                secured,
                Duration::from_secs(8),
            )
            .await?;
        if let Some(body) = report {
            if body.contains("print_error") || body.contains("\"result\":\"fail\"") {
                return Err(DeviceError::Message(format!(
                    "printer rejected print: {body}"
                )));
            }
        }
        Ok(())
    }

    async fn ams(&self) -> Result<AmsState, DeviceError> {
        let (_, ams) = lan_mqtt::fetch_status(
            &self.host,
            &self.access_code,
            &self.serial,
            Duration::from_secs(8),
        )
        .await
        .map_err(Self::map_err)?;
        ams.ok_or(DeviceError::Message(
            "push_status had no AMS block (external spool or older firmware)".into(),
        ))
    }

    async fn camera_frame(&self) -> Result<Frame, DeviceError> {
        camera::snapshot_frame(&self.host, &self.access_code).map_err(Self::map_err)
    }

    async fn pause(&self) -> Result<(), DeviceError> {
        self.command(pause(next_sequence_id())).await
    }

    async fn resume(&self) -> Result<(), DeviceError> {
        self.command(resume(next_sequence_id())).await
    }

    async fn stop(&self) -> Result<(), DeviceError> {
        self.command(stop(next_sequence_id())).await
    }

    async fn set_print_speed(&self, level: u8) -> Result<(), DeviceError> {
        self.command(print_speed(next_sequence_id(), level)).await
    }

    async fn set_chamber_light(&self, on: bool) -> Result<(), DeviceError> {
        self.command(chamber_light(next_sequence_id(), on)).await
    }

    async fn set_bed_temp(&self, temp_c: u16) -> Result<(), DeviceError> {
        self.command(set_bed_temp(next_sequence_id(), temp_c)).await
    }

    async fn set_nozzle_temp(&self, temp_c: u16) -> Result<(), DeviceError> {
        self.command(set_nozzle_temp(next_sequence_id(), temp_c))
            .await
    }

    async fn set_fan(&self, fan_index: u8, speed: u8) -> Result<(), DeviceError> {
        self.command(set_fan(next_sequence_id(), fan_index, speed))
            .await
    }

    async fn ams_load(
        &self,
        ams_id: u8,
        slot_id: u8,
        old_temp: u16,
        new_temp: u16,
    ) -> Result<(), DeviceError> {
        self.command(ams_change_filament(
            next_sequence_id(),
            true,
            ams_id,
            slot_id,
            old_temp,
            new_temp,
        ))
        .await
    }

    async fn ams_unload(&self, ams_id: u8) -> Result<(), DeviceError> {
        self.command(ams_change_filament(
            next_sequence_id(),
            false,
            ams_id,
            255,
            0,
            0,
        ))
        .await
    }

    async fn ams_drying(
        &self,
        ams_id: u8,
        filament: &str,
        temp_c: u16,
        duration_h: u16,
        rotate_tray: bool,
        cooling_temp: u16,
    ) -> Result<(), DeviceError> {
        self.command(ams_filament_drying(
            next_sequence_id(),
            ams_id,
            AMS_DRY_MODE_ON_TIME,
            filament,
            temp_c,
            duration_h,
            rotate_tray,
            cooling_temp,
        ))
        .await
    }

    async fn ams_drying_stop(&self, ams_id: u8) -> Result<(), DeviceError> {
        self.command(ams_filament_drying(
            next_sequence_id(),
            ams_id,
            AMS_DRY_MODE_OFF,
            "",
            0,
            0,
            false,
            0,
        ))
        .await
    }

    async fn nozzle_holder_ctrl(&self, action: u8) -> Result<(), DeviceError> {
        self.command(crate::mqtt::nozzle_holder_ctrl(next_sequence_id(), action))
            .await
    }

    async fn holder_nozzle_refresh(&self, id: u32) -> Result<(), DeviceError> {
        self.command(crate::mqtt::holder_nozzle_refresh(next_sequence_id(), id))
            .await
    }

    async fn nozzle_info_confirm(&self, id: u32) -> Result<(), DeviceError> {
        self.command(crate::mqtt::nozzle_info_confirm(next_sequence_id(), id))
            .await
    }

    async fn hms_resume(&self, err: &str, job_id: &str) -> Result<(), DeviceError> {
        self.command(hms_resume(next_sequence_id(), err, job_id))
            .await
    }

    async fn hms_ignore(&self, err: &str, job_id: &str) -> Result<(), DeviceError> {
        self.command(hms_ignore(next_sequence_id(), err, job_id))
            .await
    }

    async fn hms_stop(&self, err: &str, job_id: &str) -> Result<(), DeviceError> {
        self.command(hms_stop(next_sequence_id(), err, job_id))
            .await
    }

    async fn skip_objects(&self, ids: &[u32]) -> Result<(), DeviceError> {
        self.command(skip_objects(next_sequence_id(), ids)).await
    }
}

/// Publish a signed or unsigned MQTT `gcode_line`.
pub async fn send_gcode_line(
    backend: &LanBackend,
    line: &str,
) -> Result<Option<String>, ProtocolError> {
    let (state, _) = lan_mqtt::fetch_status(
        &backend.host,
        &backend.access_code,
        &backend.serial,
        Duration::from_secs(8),
    )
    .await?;
    let payload = gcode_line(next_sequence_id(), line);
    let device_cert = backend.device_cert_pem();
    Ok(lan_mqtt::publish_signed(lan_mqtt::PublishRequest {
        host: &backend.host,
        access_code: &backend.access_code,
        serial: &backend.serial,
        payload: &payload,
        creds: &backend.credentials,
        device_cert_pem: device_cert.as_deref(),
        secured: !state.developer_mode,
        wait_report: Duration::from_secs(5),
    })
    .await?)
}

pub async fn install_app_cert(backend: &LanBackend) -> Result<Option<String>, ProtocolError> {
    if !backend.credentials.can_install_app_cert() {
        return Err(ProtocolError::Credential(CredentialError::Message(
            "need slicer_cert.pem and slicer_crl.pem for app_cert_install".into(),
        )));
    }
    let payload = app_cert_install(
        next_sequence_id(),
        backend.credentials.cert_pem.as_deref().unwrap_or(""),
        backend.credentials.crl_pem.as_deref().unwrap_or(""),
    );
    let report = lan_mqtt::publish_signed(lan_mqtt::PublishRequest {
        host: &backend.host,
        access_code: &backend.access_code,
        serial: &backend.serial,
        payload: &payload,
        creds: &backend.credentials,
        device_cert_pem: None,
        secured: false,
        wait_report: Duration::from_secs(8),
    })
    .await?;
    if let Some(body) = &report {
        if let Some(pem) = parse_printer_cert(body) {
            let serial = backend.resolved_serial();
            if !serial.is_empty() {
                save_device_cert(default_config_dir(), &serial, &pem)?;
            }
        }
    }
    Ok(report)
}
