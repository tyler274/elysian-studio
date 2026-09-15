//! Studio `BambuTunnelAgora` as Agora Video SDK 4.2 audience subscribe.
//!
//! Call order matches IRtcEngine 4.2 / Server Gateway: `createAgoraRtcEngine`
//! → `initialize` → `enableEncryptionEx` (AES_128_GCM2) → `joinChannelEx`
//! (audience) → encoded observer → RDT hello. Encoded H.264 is decoded with
//! OpenH264. This crate does not load `libBambuSource` or `libagora_rtc_sdk`.

use base64::Engine;
use openh264::decoder::Decoder;
use openh264::formats::YUVSource;

use elysian_device::Frame;

use crate::camera::CameraError;
use crate::cloud_api::{CameraCreds, SLICER_CLIENT_VERSION};
use crate::oauth::percent_encode_query;

/// `agora::rtc::AREA_CODE` (`IAgoraRtcEngine.h`).
pub const AREA_CODE_NA: u32 = 0x0000_0001;
pub const AREA_CODE_AS: u32 = 0x0000_0002;
pub const AREA_CODE_EU: u32 = 0x0000_0004;
pub const AREA_CODE_CN: u32 = 0x0000_0008;
pub const AREA_CODE_JP: u32 = 0x0000_0010;
pub const AREA_CODE_IN: u32 = 0x0000_0020;
pub const AREA_CODE_GLOB: u32 = 0xFFFF_FFFF;

/// `CHANNEL_PROFILE_LIVE_BROADCASTING`.
pub const CHANNEL_PROFILE_LIVE_BROADCASTING: u32 = 1;
/// `CLIENT_ROLE_BROADCASTER`.
pub const CLIENT_ROLE_BROADCASTER: u32 = 1;
/// `CLIENT_ROLE_AUDIENCE` (`BECOME_AUDIENCE` in BambuSource).
pub const CLIENT_ROLE_AUDIENCE: u32 = 2;
/// `ENCRYPTION_MODE::AES_128_GCM2` (key + 32-byte salt).
pub const ENCRYPTION_AES_128_GCM2: u32 = 7;
/// `VIDEO_CODEC_H264`.
pub const VIDEO_CODEC_H264: u32 = 2;
/// `VIDEO_FRAME_TYPE_KEY_FRAME`.
pub const VIDEO_FRAME_TYPE_KEY_FRAME: u32 = 3;
/// `VIDEO_FRAME_TYPE_DELTA_FRAME`.
pub const VIDEO_FRAME_TYPE_DELTA_FRAME: u32 = 4;

/// Join parameters minted by `get_camera_url` / `bambu:///agora?...`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AgoraJoin {
    pub app_id: String,
    pub channel: String,
    pub token: String,
    pub region: String,
    pub user: String,
    pub stream_key: String,
    pub stream_salt: String,
    pub authkey: String,
    pub cli_id: String,
    pub cli_ver: String,
}

impl AgoraJoin {
    pub fn from_creds(creds: &CameraCreds) -> Result<Self, CameraError> {
        if creds.channel.is_empty() {
            return Err(CameraError::Message(
                "agora rtc: ttcode missing channel_name".into(),
            ));
        }
        let token = if creds.token.is_empty() {
            return Err(CameraError::Message(
                "agora rtc: ttcode missing token".into(),
            ));
        } else {
            creds.token.clone()
        };
        let app_id = if !creds.app_id.is_empty() {
            creds.app_id.clone()
        } else if let Some(id) = app_id_from_token(&token) {
            id
        } else {
            return Err(CameraError::Message(
                "agora rtc: ttcode missing app_id/license".into(),
            ));
        };
        Ok(Self {
            app_id,
            channel: creds.channel.clone(),
            token,
            region: creds.region.clone(),
            user: creds.user.clone(),
            stream_key: if creds.stream_key.is_empty() {
                creds.authkey.clone()
            } else {
                creds.stream_key.clone()
            },
            stream_salt: creds.stream_salt.clone(),
            authkey: creds.authkey.clone(),
            cli_id: String::new(),
            cli_ver: String::new(),
        })
    }

    pub fn encryption_enabled(&self) -> bool {
        !self.stream_key.is_empty() || !self.stream_salt.is_empty()
    }

    pub fn local_uid(&self) -> u32 {
        self.user.parse().unwrap_or(0)
    }

    pub fn rdt_pid(&self) -> &str {
        if self.cli_id.is_empty() {
            "bambu-studio-rs"
        } else {
            &self.cli_id
        }
    }

    pub fn rdt_ver(&self) -> &str {
        if self.cli_ver.is_empty() {
            SLICER_CLIENT_VERSION
        } else {
            &self.cli_ver
        }
    }
}

/// `RtcEngineContext` / `AgoraServiceConfiguration`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RtcEngineConfig {
    pub app_id: String,
    pub area_code: u32,
    pub channel_profile: u32,
    pub enable_video: bool,
    pub enable_audio_device: bool,
}

/// `RtcConnection` for `joinChannelEx`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RtcConnection {
    pub channel_id: String,
    pub local_uid: u32,
}

/// `ChannelMediaOptions` — audience subscribe, no publish.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ChannelMediaOptions {
    pub client_role: u32,
    pub auto_subscribe_video: bool,
    pub auto_subscribe_audio: bool,
    pub publish_camera_track: bool,
    pub publish_microphone_track: bool,
}

impl Default for ChannelMediaOptions {
    fn default() -> Self {
        Self {
            client_role: CLIENT_ROLE_AUDIENCE,
            auto_subscribe_video: true,
            auto_subscribe_audio: false,
            publish_camera_track: false,
            publish_microphone_track: false,
        }
    }
}

/// `EncryptionConfig` for `enableEncryptionEx`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct EncryptionConfig {
    pub mode: u32,
    pub key: String,
    pub salt: Vec<u8>,
}

/// `EncodedVideoFrameInfo` for `OnEncodedVideoImageReceived`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct EncodedVideoFrameInfo {
    pub codec: u32,
    pub frame_type: u32,
}

impl EncodedVideoFrameInfo {
    pub fn h264_keyframe() -> Self {
        Self {
            codec: VIDEO_CODEC_H264,
            frame_type: VIDEO_FRAME_TYPE_KEY_FRAME,
        }
    }
}

/// Prepared 4.2 audience session (no VOS/SD-RTN socket yet).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RtcSession {
    pub engine: RtcEngineConfig,
    pub connection: RtcConnection,
    pub options: ChannelMediaOptions,
    pub encryption: Option<EncryptionConfig>,
    pub token: String,
    pub rdt_hello: String,
}

impl RtcSession {
    pub fn prepare(join: &AgoraJoin) -> Result<Self, CameraError> {
        if join.channel.is_empty() {
            return Err(CameraError::Message("agora rtc: empty channel".into()));
        }
        if join.app_id.is_empty() {
            return Err(CameraError::Message("agora rtc: empty app id".into()));
        }
        if join.token.is_empty() {
            return Err(CameraError::Message("agora rtc: empty token".into()));
        }
        Ok(Self {
            engine: RtcEngineConfig {
                app_id: join.app_id.clone(),
                area_code: agora_area_code(&join.region),
                channel_profile: CHANNEL_PROFILE_LIVE_BROADCASTING,
                enable_video: true,
                enable_audio_device: false,
            },
            connection: RtcConnection {
                channel_id: join.channel.clone(),
                local_uid: join.local_uid(),
            },
            options: ChannelMediaOptions::default(),
            encryption: encryption_config(join),
            token: join.token.clone(),
            rdt_hello: agora_rdt_hello(join.rdt_pid(), join.rdt_ver()),
        })
    }

    /// BambuTunnelAgora / IRtcEngine 4.2 order (Server Gateway uses the same
    /// initialize → connect → subscribe-encoded → RDT hello sequence).
    pub fn engine_steps(&self) -> Vec<&'static str> {
        let mut steps = vec!["createAgoraRtcEngine", "initialize"];
        if self.encryption.is_some() {
            steps.push("enableEncryptionEx");
        }
        steps.extend([
            "joinChannelEx",
            "registerVideoEncodedFrameObserver",
            "sendRdtMessage",
        ]);
        steps
    }
}

/// BambuSource `initAgora region=` / Agora `AREA_CODE`.
pub fn agora_area_code(region: &str) -> u32 {
    match region.trim().to_ascii_lowercase().as_str() {
        "cn" | "china" => AREA_CODE_CN,
        "eu" | "europe" => AREA_CODE_EU,
        "asia" | "as" | "ap" => AREA_CODE_AS,
        "jp" | "japan" => AREA_CODE_JP,
        "in" | "india" => AREA_CODE_IN,
        "app" | "na" | "us" | "" => AREA_CODE_NA,
        _ => AREA_CODE_GLOB,
    }
}

/// Access-point hosts the 4.2 VocsClient / web geofence list talks to.
pub fn agora_ap_hosts(area: u32) -> &'static [&'static str] {
    crate::agora_ap::webcs_hosts(area)
}

pub fn encryption_config(join: &AgoraJoin) -> Option<EncryptionConfig> {
    if !join.encryption_enabled() {
        return None;
    }
    Some(EncryptionConfig {
        mode: ENCRYPTION_AES_128_GCM2,
        key: join.stream_key.clone(),
        salt: encryption_salt(&join.stream_salt),
    })
}

/// Agora web/native GCM2 salt: Base64 → 32 bytes (zero-padded).
pub fn encryption_salt(raw: &str) -> Vec<u8> {
    let mut bytes = base64::engine::general_purpose::STANDARD
        .decode(raw.trim())
        .unwrap_or_else(|_| raw.as_bytes().to_vec());
    bytes.resize(32, 0);
    bytes
}

/// AgoraIO Tools `006` token: version + 32-char app id.
pub fn app_id_from_token(token: &str) -> Option<String> {
    let t = token.trim();
    if t.starts_with("006") && t.len() >= 35 {
        Some(t[3..35].to_string())
    } else {
        None
    }
}

/// BambuSource RDT hello after `onJoinChannelSuccess`.
pub fn agora_rdt_hello(pid: &str, ver: &str) -> String {
    format!(
        r#"{{"sequence":0,"req":{{"t_av":1,"mtype":1,"peer_t":1,"pid":"{pid}","ver":"{ver}"}}}}"#
    )
}

pub fn parse_agora_url(url: &str) -> Result<AgoraJoin, CameraError> {
    let rest = url
        .strip_prefix("bambu:///agora?")
        .ok_or_else(|| CameraError::Message("not a bambu:///agora URL".into()))?;
    let mut join = AgoraJoin {
        app_id: String::new(),
        channel: String::new(),
        token: String::new(),
        region: String::new(),
        user: String::new(),
        stream_key: String::new(),
        stream_salt: String::new(),
        authkey: String::new(),
        cli_id: String::new(),
        cli_ver: String::new(),
    };
    for pair in rest.split('&') {
        let Some((k, v)) = pair.split_once('=') else {
            continue;
        };
        let v = percent_decode_query(v);
        match k {
            "channel" => join.channel = v,
            "region" => join.region = v,
            "token" => join.token = v,
            "authkey" => join.authkey = v,
            "license" | "app_id" => join.app_id = v,
            "user" => join.user = v,
            "streamKey" | "stream_key" => join.stream_key = v,
            "streamSalt" | "stream_salt" => join.stream_salt = v,
            "cli_id" => join.cli_id = v,
            "cli_ver" => join.cli_ver = v,
            _ => {}
        }
    }
    if join.channel.is_empty() {
        return Err(CameraError::Message("agora url missing channel".into()));
    }
    if join.app_id.is_empty() {
        if let Some(id) = app_id_from_token(&join.token) {
            join.app_id = id;
        }
    }
    Ok(join)
}

fn percent_decode_query(s: &str) -> String {
    let bytes = s.as_bytes();
    let mut out = Vec::with_capacity(bytes.len());
    let mut i = 0;
    while i < bytes.len() {
        if bytes[i] == b'%' && i + 2 < bytes.len() {
            if let (Some(h), Some(l)) = (from_hex(bytes[i + 1]), from_hex(bytes[i + 2])) {
                out.push((h << 4) | l);
                i += 3;
                continue;
            }
        }
        out.push(if bytes[i] == b'+' { b' ' } else { bytes[i] });
        i += 1;
    }
    String::from_utf8_lossy(&out).into_owned()
}

fn from_hex(b: u8) -> Option<u8> {
    match b {
        b'0'..=b'9' => Some(b - b'0'),
        b'a'..=b'f' => Some(b - b'a' + 10),
        b'A'..=b'F' => Some(b - b'A' + 10),
        _ => None,
    }
}

/// `IVideoEncodedFrameObserver::onEncodedVideoFrame` → RGBA.
pub fn on_encoded_video_frame(
    decoder: &mut Decoder,
    annexb: &[u8],
) -> Result<Option<Frame>, CameraError> {
    match decoder.decode(annexb) {
        Ok(Some(yuv)) => {
            let (width, height) = yuv.dimensions();
            let mut rgba = vec![0u8; yuv.rgba8_len()];
            yuv.write_rgba8(&mut rgba);
            Ok(Some(Frame {
                width: width as u32,
                height: height as u32,
                rgba,
            }))
        }
        Ok(None) => Ok(None),
        Err(err) => Err(CameraError::Message(err.to_string())),
    }
}

/// Server Gateway `IVideoEncodedImageReceiver::OnEncodedVideoImageReceived`.
pub fn on_encoded_video_image_received(
    decoder: &mut Decoder,
    image_buffer: &[u8],
    info: EncodedVideoFrameInfo,
) -> Result<Option<Frame>, CameraError> {
    if info.codec != VIDEO_CODEC_H264 && info.codec != 0 {
        return Err(CameraError::Message(format!(
            "agora rtc: encoded codec {} is not H.264",
            info.codec
        )));
    }
    on_encoded_video_frame(decoder, image_buffer)
}

/// Join with minted Agora creds and yield decoded chamber frames.
pub fn stream_agora_frames(
    creds: &CameraCreds,
    on_frame: impl FnMut(Frame) -> bool,
) -> Result<(), CameraError> {
    let join = AgoraJoin::from_creds(creds)?;
    tracing::debug!(
        target: "elysian_protocol::cloud",
        region = %join.region,
        channel_len = join.channel.len(),
        token_kind = %if join.token.starts_with("006") {
            "006"
        } else if join.token.starts_with("007") {
            "007"
        } else {
            "other"
        },
        encrypt = join.encryption_enabled(),
        "agora join"
    );
    let session = RtcSession::prepare(&join)?;
    if crate::agora_ap::is_agora_token(&join.token) {
        return crate::agora_ap::stream_vos_frames(&join, &session, on_frame);
    }
    let encrypt = u8::from(session.encryption.is_some());
    let _ = percent_encode_query(&join.channel);
    Err(CameraError::Message(format!(
        "agora rtc: initialize(license, area={}) joinChannelEx channel encryption={encrypt} encoded-observer ready; mint a 006/007 token for AP/VOS",
        session.engine.area_code,
    )))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::cloud_api::{camera_url_key, parse_camera_url_key, CameraProto};

    #[test]
    fn parse_studio_agora_key() {
        let key = camera_url_key("01H2C0000000001", "01.02.00.00", &["agora"], None);
        assert_eq!(key, r#"01H2C0000000001|01.02.00.00|"agora""#);
        let parsed = parse_camera_url_key(&key);
        assert_eq!(parsed.dev_id, "01H2C0000000001");
        assert_eq!(parsed.firmware.as_deref(), Some("01.02.00.00"));
        assert_eq!(parsed.protocols, vec!["agora"]);
        let refresh = parse_camera_url_key(r#"01H2C0000000001|01.02.00.00|"agora"|devchan"#);
        assert_eq!(refresh.channel.as_deref(), Some("devchan"));
        let both = parse_camera_url_key(r#"SN|fw|"tutk","agora""#);
        assert_eq!(both.protocols, vec!["tutk", "agora"]);
    }

    #[test]
    fn area_code_matches_bambusource_region_names() {
        assert_eq!(agora_area_code("us"), agora_area_code("app"));
        assert_eq!(agora_area_code("us"), AREA_CODE_NA);
        assert_eq!(agora_area_code("cn"), AREA_CODE_CN);
        assert_ne!(agora_area_code("cn"), agora_area_code("eu"));
        assert_ne!(agora_area_code("asia"), agora_area_code("us"));
        assert_eq!(CLIENT_ROLE_BROADCASTER, 1);
        assert_eq!(VIDEO_FRAME_TYPE_DELTA_FRAME, 4);
        assert!(!agora_ap_hosts(AREA_CODE_NA).is_empty());
        assert!(agora_ap_hosts(AREA_CODE_CN)
            .iter()
            .any(|h| h.contains("agoraio.cn") || h.contains("sd-rtn")));
    }

    #[test]
    fn parse_agora_query() {
        let url = "bambu:///agora?channel=ch1&region=us&token=tok%3D1&license=app&streamKey=k&streamSalt=s&user=42&cli_id=uuid&cli_ver=02.03.00.00";
        let join = parse_agora_url(url).unwrap();
        assert_eq!(join.channel, "ch1");
        assert_eq!(join.token, "tok=1");
        assert_eq!(join.app_id, "app");
        assert_eq!(join.stream_key, "k");
        assert_eq!(join.stream_salt, "s");
        assert_eq!(join.user, "42");
        assert_eq!(join.local_uid(), 42);
        assert_eq!(join.cli_id, "uuid");
        assert!(join.encryption_enabled());
        let session = RtcSession::prepare(&join).unwrap();
        assert_eq!(session.options.client_role, CLIENT_ROLE_AUDIENCE);
        assert!(session.options.auto_subscribe_video);
        assert!(!session.options.publish_camera_track);
        assert_eq!(
            session.engine_steps(),
            [
                "createAgoraRtcEngine",
                "initialize",
                "enableEncryptionEx",
                "joinChannelEx",
                "registerVideoEncodedFrameObserver",
                "sendRdtMessage",
            ]
        );
        assert_eq!(
            session.encryption.as_ref().unwrap().mode,
            ENCRYPTION_AES_128_GCM2
        );
        assert_eq!(session.encryption.as_ref().unwrap().salt.len(), 32);
        assert!(session.rdt_hello.contains("\"pid\":\"uuid\""));
    }

    #[test]
    fn rdt_hello_is_json() {
        let body = agora_rdt_hello("pid1", "1.0");
        assert!(body.contains("\"pid\":\"pid1\""));
        assert!(body.contains("\"ver\":\"1.0\""));
        assert!(body.contains("t_av"));
    }

    #[test]
    fn join_from_creds_requires_channel_token_license() {
        let mut creds = CameraCreds {
            proto: CameraProto::Agora,
            channel: "ch".into(),
            token: "tok".into(),
            app_id: "app".into(),
            stream_key: "k".into(),
            stream_salt: "s".into(),
            ..CameraCreds::default()
        };
        assert!(AgoraJoin::from_creds(&creds).unwrap().encryption_enabled());
        creds.channel.clear();
        assert!(AgoraJoin::from_creds(&creds).is_err());
    }

    #[test]
    fn six_token_supplies_app_id() {
        let token = format!("006{}rest", "a".repeat(32));
        assert_eq!(app_id_from_token(&token).unwrap().len(), 32);
        let creds = CameraCreds {
            proto: CameraProto::Agora,
            channel: "ch".into(),
            token,
            ..CameraCreds::default()
        };
        assert_eq!(AgoraJoin::from_creds(&creds).unwrap().app_id.len(), 32);
    }

    #[test]
    fn gcm2_salt_is_32_bytes() {
        let salt = encryption_salt("AQID");
        assert_eq!(salt.len(), 32);
        assert_eq!(&salt[..3], &[1, 2, 3]);
    }

    #[test]
    fn encoded_observer_constructs_decoder() {
        assert!(Decoder::new().is_ok());
        let mut decoder = Decoder::new().unwrap();
        let info = EncodedVideoFrameInfo::h264_keyframe();
        assert_eq!(info.codec, VIDEO_CODEC_H264);
        let _ = on_encoded_video_image_received(&mut decoder, &[], info);
    }
}
