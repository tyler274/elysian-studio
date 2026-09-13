//! Optional cloud MQTT + HTTPS file-upload print.
//!
//! Does **not** dlopen `libbambu_networking` and does **not** ship PEMs.
//! Tokens live under `$XDG_CONFIG_HOME/bambu-studio-rs` (never logged).

use std::path::{Path, PathBuf};
use std::time::Duration;

use bambu_device::{AmsState, DeviceError, Frame, MachineState, PrintJob, PrinterBackend};

use crate::cloud_api::{md5_hex, CloudApi};
use crate::credentials::{default_config_dir, CredentialError};
use crate::lan_mqtt::{self, BrokerAuth};
use crate::mqtt::{
    chamber_light, next_sequence_id, pause, print_speed, project_file_cloud, resume, stop,
    LAN_MQTT_PORT,
};
use crate::pack::{pack_gcode_3mf, sanitize_remote_name};

/// Public Bambu cloud MQTT brokers (OpenBambuAPI / Home Assistant Bambu Lab).
pub fn cloud_mqtt_host(region: &str) -> &'static str {
    match region.trim().to_ascii_lowercase().as_str() {
        "cn" | "china" => "cn.mqtt.bambulab.com",
        "eu" | "europe" => "eu.mqtt.bambulab.com",
        _ => "us.mqtt.bambulab.com",
    }
}

#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct CloudSession {
    pub region: String,
    pub user_id: String,
    pub access_token: String,
    pub refresh_token: String,
    pub serial: String,
}

impl CloudSession {
    pub fn is_ready(&self) -> bool {
        !self.user_id.is_empty() && !self.access_token.is_empty() && !self.serial.is_empty()
    }

    pub fn has_bearer(&self) -> bool {
        !self.access_token.is_empty()
    }

    pub fn status_lines(&self) -> Vec<String> {
        vec![
            format!(
                "cloud_user: {}",
                if self.user_id.is_empty() {
                    "missing"
                } else {
                    "present"
                }
            ),
            format!(
                "cloud_token: {}",
                if self.access_token.is_empty() {
                    "missing"
                } else {
                    "present"
                }
            ),
            format!(
                "cloud_refresh: {}",
                if self.refresh_token.is_empty() {
                    "missing"
                } else {
                    "present"
                }
            ),
            format!(
                "cloud_region: {}",
                if self.region.is_empty() {
                    "us (default)"
                } else {
                    self.region.as_str()
                }
            ),
            format!(
                "cloud_serial: {}",
                if self.serial.is_empty() {
                    "missing"
                } else {
                    "present"
                }
            ),
            format!("cloud MQTT: {}", cloud_mqtt_host(&self.region)),
        ]
    }
}

fn read_trim(path: impl AsRef<Path>) -> Result<Option<String>, CredentialError> {
    match std::fs::read_to_string(path.as_ref()) {
        Ok(s) => {
            let t = s.trim().to_string();
            Ok(if t.is_empty() { None } else { Some(t) })
        }
        Err(err) if err.kind() == std::io::ErrorKind::NotFound => Ok(None),
        Err(err) => Err(err.into()),
    }
}

fn write_if_nonempty(path: impl AsRef<Path>, value: &str) -> Result<(), CredentialError> {
    if value.is_empty() {
        return Ok(());
    }
    std::fs::write(path.as_ref(), format!("{value}\n"))?;
    Ok(())
}

pub fn load_cloud_session(dir: impl AsRef<Path>) -> Result<CloudSession, CredentialError> {
    let dir = dir.as_ref();
    Ok(CloudSession {
        region: read_trim(dir.join("cloud_region"))?.unwrap_or_default(),
        user_id: read_trim(dir.join("cloud_user"))?.unwrap_or_default(),
        access_token: read_trim(dir.join("cloud_token"))?.unwrap_or_default(),
        refresh_token: read_trim(dir.join("cloud_refresh"))?.unwrap_or_default(),
        serial: read_trim(dir.join("cloud_serial"))?.unwrap_or_default(),
    })
}

pub fn load_cloud_session_default() -> Result<CloudSession, CredentialError> {
    load_cloud_session(default_config_dir())
}

pub fn save_cloud_session(
    dir: impl AsRef<Path>,
    session: &CloudSession,
) -> Result<(), CredentialError> {
    let dir = dir.as_ref();
    std::fs::create_dir_all(dir)?;
    write_if_nonempty(dir.join("cloud_region"), &session.region)?;
    write_if_nonempty(dir.join("cloud_user"), &session.user_id)?;
    write_if_nonempty(dir.join("cloud_token"), &session.access_token)?;
    write_if_nonempty(dir.join("cloud_refresh"), &session.refresh_token)?;
    write_if_nonempty(dir.join("cloud_serial"), &session.serial)?;
    Ok(())
}

#[derive(Debug, Clone)]
pub struct CloudBackend {
    pub session: CloudSession,
    pub ams_mapping: Vec<i32>,
    pub config_dir: PathBuf,
}

impl CloudBackend {
    pub fn new(session: CloudSession) -> Self {
        Self {
            session,
            ams_mapping: Vec::new(),
            config_dir: default_config_dir(),
        }
    }

    pub fn from_config_dir(dir: impl AsRef<Path>) -> Result<Self, DeviceError> {
        let dir = dir.as_ref().to_path_buf();
        let session =
            load_cloud_session(&dir).map_err(|err| DeviceError::Message(err.to_string()))?;
        if !session.is_ready() {
            return Err(DeviceError::Message(
                "cloud MQTT needs cloud_user, cloud_token, and cloud_serial in the config dir"
                    .into(),
            ));
        }
        Ok(Self {
            session,
            ams_mapping: Vec::new(),
            config_dir: dir,
        })
    }

    pub fn with_ams_mapping(mut self, mapping: Vec<i32>) -> Self {
        self.ams_mapping = mapping;
        self
    }

    fn auth(&self) -> BrokerAuth<'_> {
        BrokerAuth {
            host: cloud_mqtt_host(&self.session.region),
            port: LAN_MQTT_PORT,
            user: self.session.user_id.as_str(),
            password: self.session.access_token.as_str(),
            serial: self.session.serial.as_str(),
        }
    }

    fn map_err(err: impl std::fmt::Display) -> DeviceError {
        DeviceError::Message(err.to_string())
    }

    async fn publish_cmd(&self, payload: &str) -> Result<(), DeviceError> {
        lan_mqtt::publish_raw(self.auth(), payload.as_bytes(), Duration::from_secs(5))
            .await
            .map_err(Self::map_err)?;
        Ok(())
    }
}

impl PrinterBackend for CloudBackend {
    async fn status(&self) -> Result<MachineState, DeviceError> {
        let (mut state, _) = lan_mqtt::fetch_status_on(self.auth(), Duration::from_secs(8))
            .await
            .map_err(Self::map_err)?;
        if state.serial.is_empty() {
            state.serial = self.session.serial.clone();
        }
        Ok(state)
    }

    async fn start_print(&self, job: PrintJob) -> Result<(), DeviceError> {
        if !self.session.has_bearer() {
            return Err(DeviceError::Message(
                "cloud upload needs a Bearer token (Import Studio or device login)".into(),
            ));
        }
        let stem = sanitize_remote_name(&job.filename);
        let remote = format!("{stem}.gcode.3mf");
        let archive = pack_gcode_3mf(&job.gcode).map_err(Self::map_err)?;
        let hash = md5_hex(&archive);
        let mut session = self.session.clone();
        let mut api = CloudApi::new(
            &session.region,
            &session.access_token,
            &session.refresh_token,
        );
        let ticket = api
            .with_retry(|api| api.upload_file(&remote, &archive))
            .map_err(Self::map_err)?;
        session.access_token = api.access_token;
        session.refresh_token = api.refresh_token;
        let _ = save_cloud_session(&self.config_dir, &session);
        if !ticket.public_url.starts_with("https://") {
            return Err(DeviceError::Message(
                "cloud upload did not return an https file URL".into(),
            ));
        }
        tracing::info!(
            "cloud upload {} ({} bytes) → MQTT project_file",
            remote,
            archive.len()
        );
        let payload = project_file_cloud(
            next_sequence_id(),
            &remote,
            &stem,
            1,
            &ticket.public_url,
            &hash,
            &self.ams_mapping,
        );
        lan_mqtt::publish_raw(
            BrokerAuth {
                host: cloud_mqtt_host(&session.region),
                port: LAN_MQTT_PORT,
                user: session.user_id.as_str(),
                password: session.access_token.as_str(),
                serial: session.serial.as_str(),
            },
            payload.as_bytes(),
            Duration::from_secs(8),
        )
        .await
        .map_err(Self::map_err)?;
        Ok(())
    }

    async fn ams(&self) -> Result<AmsState, DeviceError> {
        let (_, ams) = lan_mqtt::fetch_status_on(self.auth(), Duration::from_secs(8))
            .await
            .map_err(Self::map_err)?;
        ams.ok_or(DeviceError::Message(
            "push_status had no AMS block (external spool or older firmware)".into(),
        ))
    }

    async fn camera_frame(&self) -> Result<Frame, DeviceError> {
        Err(DeviceError::NotImplemented)
    }

    async fn pause(&self) -> Result<(), DeviceError> {
        self.publish_cmd(&pause(next_sequence_id())).await
    }

    async fn resume(&self) -> Result<(), DeviceError> {
        self.publish_cmd(&resume(next_sequence_id())).await
    }

    async fn stop(&self) -> Result<(), DeviceError> {
        self.publish_cmd(&stop(next_sequence_id())).await
    }

    async fn set_print_speed(&self, level: u8) -> Result<(), DeviceError> {
        self.publish_cmd(&print_speed(next_sequence_id(), level))
            .await
    }

    async fn set_chamber_light(&self, on: bool) -> Result<(), DeviceError> {
        self.publish_cmd(&chamber_light(next_sequence_id(), on))
            .await
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn region_maps_to_public_broker() {
        assert_eq!(cloud_mqtt_host("us"), "us.mqtt.bambulab.com");
        assert_eq!(cloud_mqtt_host("EU"), "eu.mqtt.bambulab.com");
        assert_eq!(cloud_mqtt_host("cn"), "cn.mqtt.bambulab.com");
    }

    #[test]
    fn session_files_roundtrip() {
        let dir = std::env::temp_dir().join(format!("bambu-cloud-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        std::fs::write(dir.join("cloud_user"), "12345\n").unwrap();
        std::fs::write(dir.join("cloud_token"), "tok\n").unwrap();
        std::fs::write(dir.join("cloud_refresh"), "ref\n").unwrap();
        std::fs::write(dir.join("cloud_region"), "eu\n").unwrap();
        std::fs::write(dir.join("cloud_serial"), "01S\n").unwrap();
        let session = load_cloud_session(&dir).unwrap();
        assert!(session.is_ready());
        assert_eq!(session.region, "eu");
        assert_eq!(session.user_id, "12345");
        assert_eq!(session.refresh_token, "ref");
        assert_eq!(cloud_mqtt_host(&session.region), "eu.mqtt.bambulab.com");
        let mut copy = session.clone();
        copy.serial = "01T".into();
        save_cloud_session(&dir, &copy).unwrap();
        let loaded = load_cloud_session(&dir).unwrap();
        assert_eq!(loaded.serial, "01T");
        let _ = std::fs::remove_dir_all(&dir);
    }
}
