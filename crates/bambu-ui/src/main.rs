#![forbid(unsafe_code)]

mod monitor;
mod sidebar;

use bambu_alloc as _;
use std::path::PathBuf;
use std::process::Command;

use bambu_config::{
    list_bbl_profiles, load_bbl_process, overlay_bbl_profile, BblProfileEntry, BblProfileKind,
    SliceSettings,
};
use bambu_device::{AmsState, MachineState, PrintJob, PrinterBackend};
use bambu_gcode::{parse_gcode, write_gcode, write_gcode_for_objects};
use bambu_gpu::{
    force_vulkan_env, paint_overlay_color, probe_vulkan, slice_volumes_with_gpu_or_cpu,
    slice_with_gpu_or_cpu, ExtrusionRole, ToolpathBuffer, ViewportEvent, ViewportScene,
};
use bambu_io::{load_mesh, load_model};
use bambu_model::{Model, TrianglePaint};
use bambu_protocol::{
    describe_hms, load_cached_catalog, load_cloud_session, load_lan_codes, refresh_catalog,
    save_cloud_session, CloudApi, CloudBackend, CloudDevice, LanBackend, LoginResult,
    StudioPrinter,
};
use bambu_slicer::check_print_path_conflicts;
use iced::widget::{button, checkbox, column, container, row, shader, text};
use iced::{Color, Element, Fill, Subscription, Task, Theme};

use monitor::JpegThumb;

fn main() -> iced::Result {
    reexec_with_vulkan_if_needed();

    tracing_subscriber::fmt()
        .with_env_filter(
            tracing_subscriber::EnvFilter::from_default_env()
                .add_directive("bambu_ui=info".parse().unwrap())
                .add_directive("bambu_gpu=info".parse().unwrap()),
        )
        .init();

    force_vulkan_env();
    let report = probe_vulkan().unwrap_or_else(|err| {
        panic!("Vulkan adapter required on this host: {err}");
    });

    #[cfg(target_os = "linux")]
    if !report.is_vulkan {
        panic!(
            "expected Vulkan backend, got {} ({})",
            report.backend, report.name
        );
    }

    tracing::info!("GPU adapter: {} backend={}", report.name, report.backend);

    let adapter = format!("{} / {}", report.backend, report.name);
    iced::application(move || App::new(adapter.clone()), App::update, App::view)
        .subscription(App::subscription)
        .title("Bambu Studio")
        .theme(Theme::Dark)
        .antialiasing(true)
        .run()
}

fn reexec_with_vulkan_if_needed() {
    if std::env::var("WGPU_BACKEND").ok().as_deref() == Some("vulkan") {
        return;
    }

    let current_exe = std::env::current_exe().expect("current_exe");
    let mut cmd = Command::new(&current_exe);
    cmd.env("WGPU_BACKEND", "vulkan")
        .args(std::env::args_os().skip(1));

    #[cfg(unix)]
    {
        use std::os::unix::process::CommandExt;
        let err = cmd.exec();
        panic!("failed to re-exec with WGPU_BACKEND=vulkan: {err}");
    }

    #[cfg(not(unix))]
    {
        let status = cmd.status().expect("re-exec with WGPU_BACKEND=vulkan");
        std::process::exit(status.code().unwrap_or(1));
    }
}

struct App {
    adapter: String,
    scene: ViewportScene,
    status: String,
    host: String,
    access_code: String,
    serial: String,
    last_gcode: Option<String>,
    estimated_seconds: Option<f64>,
    settings: SliceSettings,
    model: Option<Model>,
    plate: usize,
    by_object: bool,
    paint_kind: Option<PaintKind>,
    paint_blocker: bool,
    brush_mm: f32,
    mqtt_status: String,
    process_profiles: Vec<BblProfileEntry>,
    filament_profiles: Vec<BblProfileEntry>,
    machine_profiles: Vec<BblProfileEntry>,
    process_name: Option<String>,
    filament_name: Option<String>,
    machine_name: Option<String>,
    selected_object: usize,
    selected_volume: usize,
    machine: MachineState,
    ams: AmsState,
    hms_lines: Vec<String>,
    live_monitor: bool,
    send_via: SendVia,
    cloud_user: String,
    cloud_region: String,
    has_bearer: bool,
    imported_printers: Vec<StudioPrinter>,
    cloud_devices: Vec<CloudDevice>,
    selected_device: Option<String>,
    login_account: String,
    login_password: String,
    login_code: String,
    chamber_thumb: Option<JpegThumb>,
    camera_note: String,
}

#[derive(Debug, Clone)]
enum Message {
    Viewport(ViewportEvent),
    OpenModel,
    Slice,
    ResetCamera,
    ExtractKeys,
    Discover,
    Discovered(Result<Vec<bambu_protocol::DiscoveredPrinter>, String>),
    Host(String),
    AccessCode(String),
    Serial(String),
    Send,
    Sent(Result<(), String>),
    Chamber,
    ChamberShot(Result<ChamberResult, String>),
    WallLoops(u32),
    Infill(f64),
    LayerHeight(f64),
    NozzleTemp(f64),
    Plate(usize),
    PreviewLayer(u32),
    HideInfill(bool),
    HideSupport(bool),
    ByObject(bool),
    PaintSupport,
    PaintSeam,
    PaintFuzzy,
    PaintClear,
    PaintBlocker(bool),
    BrushRadius(f32),
    PreviewMove(u32),
    EnableSupport(bool),
    RefreshStatus,
    Status(Result<Box<MonitorSnapshot>, String>),
    Pause,
    Resume,
    Stop,
    PrintControl(Result<String, String>),
    LiveMonitor(bool),
    SendVia(SendVia),
    ImportStudio,
    StudioImported(Result<Box<StudioImportUi>, String>),
    RefreshDevices,
    DevicesLoaded(Result<Vec<CloudDevice>, String>),
    PickDevice(String),
    LoginAccount(String),
    LoginPassword(String),
    LoginCode(String),
    CloudLogin,
    CloudLogged(Result<String, String>),
    PrintSpeed(u8),
    ChamberLight(bool),
    ProcessProfile(String),
    FilamentProfile(String),
    MachineProfile(String),
    SelectObject(usize),
    SelectVolume(usize),
    HideVolume(bool),
    CycleAmsMap(usize),
    RefreshHms,
    HmsCatalog(Result<String, String>),
    Calibration,
}

impl From<ViewportEvent> for Message {
    fn from(event: ViewportEvent) -> Self {
        Message::Viewport(event)
    }
}

#[derive(Debug, Clone)]
struct MonitorSnapshot {
    line: String,
    machine: MachineState,
    ams: AmsState,
    hms_lines: Vec<String>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
enum SendVia {
    #[default]
    LanFtps,
    CloudUpload,
}

impl SendVia {
    fn label(self) -> &'static str {
        match self {
            SendVia::LanFtps => "LAN FTPS",
            SendVia::CloudUpload => "Cloud upload",
        }
    }
}

impl std::fmt::Display for SendVia {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(self.label())
    }
}

#[derive(Debug, Clone)]
enum ChamberResult {
    Jpeg { bytes: usize, thumb: JpegThumb },
    Rtsps { detail: String },
}

#[derive(Debug, Clone)]
struct StudioImportUi {
    user_id: String,
    region: String,
    has_token: bool,
    printers: Vec<StudioPrinter>,
    default_serial: String,
    status: String,
    discovered: Vec<bambu_protocol::DiscoveredPrinter>,
}

impl App {
    fn new(adapter: String) -> Self {
        let settings = default_slice_settings();
        let bed = settings.bed_size_mm();
        let scene = ViewportScene::with_cube_on_bed(adapter.clone(), bed);
        let mut app = Self {
            adapter,
            scene,
            status: format!("20mm cube on {bed:.0}mm bed"),
            host: String::new(),
            access_code: String::new(),
            serial: String::new(),
            last_gcode: None,
            estimated_seconds: None,
            settings,
            model: None,
            plate: 0,
            by_object: false,
            paint_kind: None,
            paint_blocker: false,
            brush_mm: 2.0,
            mqtt_status: String::new(),
            process_profiles: list_bbl_profiles(BblProfileKind::Process),
            filament_profiles: list_bbl_profiles(BblProfileKind::Filament),
            machine_profiles: list_bbl_profiles(BblProfileKind::Machine),
            process_name: None,
            filament_name: None,
            machine_name: None,
            selected_object: 0,
            selected_volume: 0,
            machine: MachineState::default(),
            ams: AmsState::default(),
            hms_lines: Vec::new(),
            live_monitor: false,
            send_via: SendVia::LanFtps,
            cloud_user: String::new(),
            cloud_region: String::new(),
            has_bearer: false,
            imported_printers: Vec::new(),
            cloud_devices: Vec::new(),
            selected_device: None,
            login_account: String::new(),
            login_password: String::new(),
            login_code: String::new(),
            chamber_thumb: None,
            camera_note: String::new(),
        };
        app.load_account_from_disk();
        app
    }

    fn load_account_from_disk(&mut self) {
        let dir = bambu_protocol::default_config_dir();
        if let Ok(session) = load_cloud_session(&dir) {
            self.cloud_user = session.user_id;
            self.cloud_region = session.region;
            self.has_bearer = !session.access_token.is_empty();
            if self.serial.is_empty() {
                self.serial = session.serial;
            }
        }
        let codes = load_lan_codes(&dir);
        if self.access_code.is_empty() {
            if let Some(code) = codes.get(&self.serial) {
                self.access_code = code.clone();
            }
        }
        self.imported_printers = codes
            .into_iter()
            .map(|(serial, access_code)| StudioPrinter {
                serial,
                access_code,
            })
            .collect();
    }

    fn apply_studio_import(&mut self, imported: StudioImportUi) {
        self.imported_printers = imported.printers;
        self.load_account_from_disk();
        if self.serial.is_empty() {
            self.serial = imported.default_serial.clone();
        }
        let serial = self.serial.clone();
        self.apply_device(&serial);
        if self.host.is_empty() {
            if let Some(found) = imported
                .discovered
                .iter()
                .find(|p| p.dev_id == serial)
                .or_else(|| imported.discovered.first())
            {
                self.host = found.dev_ip.clone();
                if self.serial.is_empty() {
                    self.serial = found.dev_id.clone();
                    let serial = self.serial.clone();
                    self.apply_device(&serial);
                }
            }
        }
        self.cloud_user = if imported.user_id.is_empty() {
            self.cloud_user.clone()
        } else {
            imported.user_id
        };
        if !imported.region.is_empty() {
            self.cloud_region = imported.region;
        }
        self.has_bearer = imported.has_token || self.has_bearer;
        self.status = imported.status;
    }

    fn apply_device(&mut self, serial: &str) {
        if serial.is_empty() {
            return;
        }
        self.serial = serial.to_string();
        self.selected_device = Some(serial.to_string());
        if let Some(p) = self.imported_printers.iter().find(|p| p.serial == serial) {
            self.access_code = p.access_code.clone();
        } else if let Some(code) = load_lan_codes(bambu_protocol::default_config_dir()).get(serial)
        {
            self.access_code = code.clone();
        }
        if let Ok(mut session) = load_cloud_session(bambu_protocol::default_config_dir()) {
            session.serial = serial.to_string();
            let _ = save_cloud_session(bambu_protocol::default_config_dir(), &session);
        }
    }

    fn subscription(&self) -> Subscription<Message> {
        if self.live_monitor {
            iced::time::every(std::time::Duration::from_secs(5)).map(|_| Message::RefreshStatus)
        } else {
            Subscription::none()
        }
    }

    fn update(&mut self, message: Message) -> Task<Message> {
        match message {
            Message::Viewport(ViewportEvent::Orbit { dx, dy }) => {
                self.scene.camera.orbit(dx, dy);
            }
            Message::Viewport(ViewportEvent::Zoom(delta)) => {
                self.scene.camera.zoom(delta);
            }
            Message::Viewport(ViewportEvent::Click {
                ndc_x,
                ndc_y,
                aspect,
            }) => {
                self.paint_pick(ndc_x, ndc_y, aspect);
            }
            Message::OpenModel => {
                if let Some(path) = rfd::FileDialog::new()
                    .add_filter("Meshes", &["3mf", "3MF", "stl", "STL"])
                    .add_filter("3MF", &["3mf", "3MF"])
                    .add_filter("STL", &["stl", "STL"])
                    .pick_file()
                {
                    match load_model(&path) {
                        Ok(model) => {
                            if let Some(s) = model.settings.clone() {
                                self.settings = s;
                            }
                            self.plate = 0;
                            let tris = model
                                .mesh_for_plate(0)
                                .map(|m| m.indices.len())
                                .unwrap_or(0);
                            if let Some(mesh) = model.mesh_for_plate(0) {
                                self.scene.set_mesh(mesh);
                            }
                            self.status = format!(
                                "loaded {} ({} triangles, {} plates)",
                                path.file_name().and_then(|n| n.to_str()).unwrap_or("mesh"),
                                tris,
                                model.plates.len().max(1)
                            );
                            self.model = Some(model);
                        }
                        Err(_) => match load_mesh(&path) {
                            Ok(mesh) => {
                                let tris = mesh.indices.len();
                                self.model = Some(Model::from_mesh(
                                    path.file_name().and_then(|n| n.to_str()).unwrap_or("mesh"),
                                    mesh.clone(),
                                ));
                                self.scene.set_mesh(mesh);
                                self.status = format!(
                                    "loaded {} ({} triangles)",
                                    path.file_name().and_then(|n| n.to_str()).unwrap_or("mesh"),
                                    tris
                                );
                            }
                            Err(err) => self.status = format!("open failed: {err}"),
                        },
                    }
                }
            }
            Message::Slice => return self.slice_current(),
            Message::ResetCamera => {
                self.scene.camera = bambu_gpu::OrbitCamera::looking_at_bed(self.scene.bed_mm);
            }
            Message::ExtractKeys => match bambu_protocol::extract_to_config_dir(None, None) {
                Ok(report) => {
                    let dir = bambu_protocol::default_config_dir();
                    self.status = format!(
                        "keys → {} · sign={} · {}",
                        dir.display(),
                        if report.credentials.can_sign() {
                            "ready"
                        } else {
                            "missing slicer_key.pem"
                        },
                        report.notes.last().cloned().unwrap_or_default()
                    );
                }
                Err(err) => self.status = format!("extract failed: {err}"),
            },
            Message::ImportStudio => {
                self.status = "Import Studio…".into();
                return Task::perform(
                    async {
                        std::thread::spawn(|| {
                            let imported = bambu_protocol::import_studio(None, None)
                                .map_err(|err| err.to_string())?;
                            let discovered =
                                bambu_protocol::discover(std::time::Duration::from_secs(3))
                                    .unwrap_or_default();
                            let status = imported
                                .status_lines()
                                .into_iter()
                                .take(8)
                                .collect::<Vec<_>>()
                                .join(" · ");
                            Ok(Box::new(StudioImportUi {
                                user_id: imported.user_id,
                                region: imported.region,
                                has_token: imported.has_token,
                                printers: imported.printers,
                                default_serial: imported.default_serial,
                                status,
                                discovered,
                            }))
                        })
                        .join()
                        .unwrap_or_else(|_| Err("import thread panicked".into()))
                    },
                    Message::StudioImported,
                );
            }
            Message::StudioImported(Ok(imported)) => {
                self.apply_studio_import(*imported);
            }
            Message::StudioImported(Err(err)) => {
                self.status = format!("import failed: {err}");
            }
            Message::RefreshDevices => {
                self.status = "cloud bind list…".into();
                return Task::perform(
                    async {
                        std::thread::spawn(|| {
                            let dir = bambu_protocol::default_config_dir();
                            let session =
                                load_cloud_session(&dir).map_err(|err| err.to_string())?;
                            if !session.has_bearer() {
                                return Err("cloud_token missing".into());
                            }
                            let mut api = CloudApi::new(
                                &session.region,
                                &session.access_token,
                                &session.refresh_token,
                            );
                            let devices = api
                                .with_retry(|api| api.list_devices())
                                .map_err(|err| err.to_string())?;
                            if api.access_token != session.access_token {
                                let mut next = session.clone();
                                next.access_token = api.access_token;
                                next.refresh_token = api.refresh_token;
                                let _ = save_cloud_session(&dir, &next);
                            }
                            Ok(devices)
                        })
                        .join()
                        .unwrap_or_else(|_| Err("devices thread panicked".into()))
                    },
                    Message::DevicesLoaded,
                );
            }
            Message::DevicesLoaded(Ok(devices)) => {
                self.cloud_devices = devices;
                self.status = format!("{} cloud device(s)", self.cloud_devices.len());
            }
            Message::DevicesLoaded(Err(err)) => {
                self.status = format!("devices: {err}");
            }
            Message::PickDevice(label) => {
                if let Some(dev) = self
                    .cloud_devices
                    .iter()
                    .find(|d| d.label() == label)
                    .cloned()
                {
                    self.apply_device(&dev.dev_id);
                }
            }
            Message::LoginAccount(s) => self.login_account = s,
            Message::LoginPassword(s) => self.login_password = s,
            Message::LoginCode(s) => self.login_code = s,
            Message::CloudLogin => {
                let account = self.login_account.clone();
                let password = self.login_password.clone();
                let code = self.login_code.clone();
                let region = if self.cloud_region.is_empty() {
                    "us".into()
                } else {
                    self.cloud_region.clone()
                };
                self.status = "cloud login…".into();
                return Task::perform(
                    async move {
                        std::thread::spawn(move || {
                            let code_opt = if code.is_empty() { None } else { Some(code) };
                            let result =
                                CloudApi::login(&region, &account, &password, code_opt.as_deref())
                                    .map_err(|err| err.to_string())?;
                            match result {
                                LoginResult::NeedsCode { login_type } => {
                                    Err(format!("enter email code ({login_type})"))
                                }
                                LoginResult::Tokens {
                                    access_token,
                                    refresh_token,
                                    user_id,
                                } => {
                                    let dir = bambu_protocol::default_config_dir();
                                    let mut session = load_cloud_session(&dir).unwrap_or_default();
                                    session.region = region;
                                    session.access_token = access_token;
                                    if !refresh_token.is_empty() {
                                        session.refresh_token = refresh_token;
                                    }
                                    if !user_id.is_empty() {
                                        session.user_id = user_id;
                                    }
                                    save_cloud_session(&dir, &session)
                                        .map_err(|err| err.to_string())?;
                                    Ok("logged in (token stored, not shown)".into())
                                }
                            }
                        })
                        .join()
                        .unwrap_or_else(|_| Err("login thread panicked".into()))
                    },
                    Message::CloudLogged,
                );
            }
            Message::CloudLogged(Ok(msg)) => {
                self.load_account_from_disk();
                self.login_password.clear();
                self.login_code.clear();
                self.status = msg;
            }
            Message::CloudLogged(Err(err)) => self.status = format!("login: {err}"),
            Message::Discover => {
                self.status = "SSDP discover on UDP 2021…".into();
                return Task::perform(
                    async {
                        std::thread::spawn(|| {
                            bambu_protocol::discover(std::time::Duration::from_secs(3))
                                .map_err(|err| err.to_string())
                        })
                        .join()
                        .unwrap_or_else(|_| Err("discover thread panicked".into()))
                    },
                    Message::Discovered,
                );
            }
            Message::Discovered(Ok(list)) if list.is_empty() => {
                self.status = "no printers on UDP 2021 (3s)".into();
            }
            Message::Discovered(Ok(list)) => {
                if let Some(first) = list.first() {
                    self.host = first.dev_ip.clone();
                    self.serial = first.dev_id.clone();
                }
                self.status = list
                    .iter()
                    .map(|p| format!("{} {}", p.dev_ip, p.dev_name))
                    .collect::<Vec<_>>()
                    .join(" · ");
            }
            Message::Discovered(Err(err)) => {
                self.status = format!("discover failed: {err}");
            }
            Message::Host(s) => self.host = s,
            Message::AccessCode(s) => self.access_code = s,
            Message::Serial(s) => self.serial = s,
            Message::Send => {
                let Some(gcode) = self.last_gcode.clone() else {
                    self.status = "slice before send".into();
                    return Task::none();
                };
                let send_via = self.send_via;
                if send_via == SendVia::LanFtps
                    && (self.host.is_empty() || self.access_code.is_empty())
                {
                    self.status = "printer IP and LAN access code required".into();
                    return Task::none();
                }
                if send_via == SendVia::CloudUpload && !self.has_bearer {
                    self.status =
                        "cloud upload needs a Bearer token (Import Studio or login)".into();
                    return Task::none();
                }
                let host = self.host.clone();
                let code = self.access_code.clone();
                let serial = self.serial.clone();
                let mapping = self.settings.filament_map.clone();
                self.status = match send_via {
                    SendVia::LanFtps => format!("FTPS + MQTT to {host}…"),
                    SendVia::CloudUpload => "cloud upload + MQTT project_file…".into(),
                };
                return Task::perform(
                    async move {
                        match send_via {
                            SendVia::CloudUpload => {
                                let backend = CloudBackend::from_config_dir(
                                    bambu_protocol::default_config_dir(),
                                )
                                .map_err(|err| err.to_string())?
                                .with_ams_mapping(mapping);
                                backend
                                    .start_print(PrintJob {
                                        filename: "plater.gcode".into(),
                                        gcode,
                                    })
                                    .await
                                    .map_err(|err| err.to_string())
                            }
                            SendVia::LanFtps => {
                                let creds = bambu_protocol::load_from_dir(
                                    bambu_protocol::default_config_dir(),
                                )
                                .unwrap_or_else(|_| Default::default());
                                let backend = LanBackend::new(host, code)
                                    .with_serial(serial)
                                    .with_credentials(creds)
                                    .with_ams_mapping(mapping);
                                backend
                                    .start_print(PrintJob {
                                        filename: "plater.gcode".into(),
                                        gcode,
                                    })
                                    .await
                                    .map_err(|err| err.to_string())
                            }
                        }
                    },
                    Message::Sent,
                );
            }
            Message::Sent(Ok(())) => {
                self.status = "print command sent".into();
            }
            Message::Sent(Err(err)) => {
                self.status = format!("send failed: {err}");
            }
            Message::Chamber => {
                if self.host.is_empty() || self.access_code.is_empty() {
                    self.status = "printer IP and LAN access code required".into();
                    return Task::none();
                }
                let host = self.host.clone();
                let code = self.access_code.clone();
                self.status = format!("chamber JPEG :6000 / RTSPS :322 {host}…");
                return Task::perform(
                    async move {
                        std::thread::spawn(move || monitor::grab_chamber(host, code))
                            .join()
                            .unwrap_or_else(|_| Err("camera thread panicked".into()))
                    },
                    Message::ChamberShot,
                );
            }
            Message::ChamberShot(Ok(ChamberResult::Jpeg { bytes, thumb })) => {
                self.status = format!(
                    "chamber JPEG {}×{} ({} bytes)",
                    thumb.width, thumb.height, bytes
                );
                self.camera_note.clear();
                self.chamber_thumb = Some(thumb);
            }
            Message::ChamberShot(Ok(ChamberResult::Rtsps { detail })) => {
                self.chamber_thumb = None;
                self.camera_note = detail.clone();
                self.status = detail;
            }
            Message::ChamberShot(Err(err)) => {
                self.status = format!("camera failed: {err}");
            }
            Message::WallLoops(n) => self.settings.wall_loops = n.clamp(1, 10),
            Message::Infill(v) => self.settings.infill_density = v.clamp(0.0, 1.0),
            Message::LayerHeight(v) => self.settings.layer_height_mm = v.clamp(0.08, 0.4),
            Message::NozzleTemp(v) => {
                self.settings.temperature_c = v.round().clamp(160.0, 300.0) as u16;
            }
            Message::Plate(i) => {
                let n = self
                    .model
                    .as_ref()
                    .map(|m| m.plates.len().max(1))
                    .unwrap_or(1);
                self.plate = i.min(n.saturating_sub(1));
                if let Some(mesh) = self
                    .model
                    .as_ref()
                    .and_then(|m| m.mesh_for_plate(self.plate))
                {
                    self.scene.set_mesh(mesh);
                }
                self.status = format!("plate {}", self.plate + 1);
            }
            Message::PreviewLayer(i) => {
                let max = self.scene.toolpaths.layer_zs.len().saturating_sub(1) as u32;
                self.scene.preview_layer = i.min(max);
                self.scene.preview_vertices = self
                    .scene
                    .toolpaths
                    .vertices_for_layer(self.scene.preview_layer as usize)
                    as u32;
            }
            Message::HideInfill(v) => self.scene.hide_infill = v,
            Message::HideSupport(v) => self.scene.hide_support = v,
            Message::ByObject(v) => self.by_object = v,
            Message::PaintSupport => {
                self.paint_kind = Some(PaintKind::Support);
                self.scene.keep_solid = true;
                self.status = "click a triangle to paint support".into();
            }
            Message::PaintSeam => {
                self.paint_kind = Some(PaintKind::Seam);
                self.scene.keep_solid = true;
                self.status = "click a triangle to paint seam".into();
            }
            Message::PaintFuzzy => {
                self.paint_kind = Some(PaintKind::Fuzzy);
                self.scene.keep_solid = true;
                self.status = "click a triangle to paint fuzzy".into();
            }
            Message::PaintClear => {
                self.paint_kind = None;
                self.scene.keep_solid = false;
                self.status = "paint mode off".into();
            }
            Message::PaintBlocker(v) => self.paint_blocker = v,
            Message::BrushRadius(v) => self.brush_mm = v.clamp(0.5, 12.0),
            Message::PreviewMove(i) => {
                self.scene.preview_vertices = i;
                self.scene.preview_layer =
                    self.scene.toolpaths.layer_index_for_vertices(i as usize) as u32;
            }
            Message::EnableSupport(v) => self.settings.enable_support = v,
            Message::RefreshStatus => return self.refresh_monitor(),
            Message::Status(Ok(snap)) => {
                self.machine = snap.machine.clone();
                self.ams = snap.ams.clone();
                self.hms_lines = snap.hms_lines.clone();
                self.mqtt_status = snap.line.clone();
                self.status = snap.line.clone();
            }
            Message::Status(Err(err)) => self.status = format!("status failed: {err}"),
            Message::Pause => return self.run_print_cmd(PrintCmd::Pause),
            Message::Resume => return self.run_print_cmd(PrintCmd::Resume),
            Message::Stop => return self.run_print_cmd(PrintCmd::Stop),
            Message::PrintSpeed(level) => return self.run_print_cmd(PrintCmd::Speed(level)),
            Message::ChamberLight(on) => return self.run_print_cmd(PrintCmd::Light(on)),
            Message::PrintControl(Ok(msg)) => {
                self.status = msg;
                return self.refresh_monitor();
            }
            Message::PrintControl(Err(err)) => self.status = format!("command failed: {err}"),
            Message::LiveMonitor(v) => {
                self.live_monitor = v;
                if v {
                    return self.refresh_monitor();
                }
            }
            Message::SendVia(v) => self.send_via = v,
            Message::ProcessProfile(name) => {
                if let Some(path) = self
                    .process_profiles
                    .iter()
                    .find(|p| p.name == name)
                    .map(|p| p.path.clone())
                {
                    match load_bbl_process(&path) {
                        Ok(s) => {
                            self.settings = s;
                            self.process_name = Some(name);
                            if let Some(fil) = self.filament_name.clone() {
                                self.apply_named_profile(BblProfileKind::Filament, &fil);
                            }
                            if let Some(mac) = self.machine_name.clone() {
                                self.apply_named_profile(BblProfileKind::Machine, &mac);
                            }
                            self.status = format!("process {path:?}");
                        }
                        Err(err) => self.status = format!("process profile: {err}"),
                    }
                }
            }
            Message::FilamentProfile(name) => {
                self.apply_named_profile(BblProfileKind::Filament, &name);
            }
            Message::MachineProfile(name) => {
                self.apply_named_profile(BblProfileKind::Machine, &name);
            }
            Message::SelectObject(i) => {
                self.selected_object = i;
                self.selected_volume = 0;
            }
            Message::SelectVolume(i) => self.selected_volume = i,
            Message::HideVolume(hidden) => {
                if let Some(vol) = self.selected_vol_mut() {
                    vol.hidden = hidden;
                    self.sync_scene_mesh();
                }
            }
            Message::CycleAmsMap(i) => {
                if self.settings.filament_map.is_empty() {
                    self.settings.filament_map = vec![1];
                }
                if let Some(slot) = self.settings.filament_map.get_mut(i) {
                    let max = self.ams.slot_count.max(4) as i32;
                    *slot = if *slot >= max { 0 } else { *slot + 1 };
                }
            }
            Message::RefreshHms => {
                self.status = "HMS catalog e.bambulab.com…".into();
                return Task::perform(
                    async {
                        std::thread::spawn(|| {
                            refresh_catalog(bambu_protocol::default_config_dir(), "en")
                                .map(|_| "HMS catalog cached".to_string())
                                .map_err(|err| err.to_string())
                        })
                        .join()
                        .unwrap_or_else(|_| Err("HMS thread panicked".into()))
                    },
                    Message::HmsCatalog,
                );
            }
            Message::HmsCatalog(Ok(msg)) => {
                self.status = msg;
                if !self.machine.hms.is_empty() {
                    let catalog = load_cached_catalog(bambu_protocol::default_config_dir(), "en");
                    self.hms_lines = self
                        .machine
                        .hms
                        .iter()
                        .map(|h| describe_hms(catalog.as_ref(), *h, "en"))
                        .collect();
                }
            }
            Message::HmsCatalog(Err(err)) => self.status = format!("HMS catalog: {err}"),
            Message::Calibration => {
                if let Some(path) = calibration_block_path() {
                    match load_model(&path) {
                        Ok(model) => {
                            self.plate = 0;
                            if let Some(mesh) = model.mesh_for_plate(0) {
                                self.scene.set_mesh(mesh);
                            }
                            self.status = format!(
                                "calibration block ({} objects) — Slice with current settings",
                                model.objects.len()
                            );
                            self.model = Some(model);
                        }
                        Err(err) => self.status = format!("calibration open failed: {err}"),
                    }
                } else {
                    self.status = "tests/calibration_block not found".into();
                }
            }
        }
        Task::none()
    }

    fn view(&self) -> Element<'_, Message> {
        let viewport = shader(&self.scene).width(Fill).height(Fill);
        row![
            container(self.sidebar())
                .style(|_| container::Style {
                    background: Some(iced::Background::Color(Color::from_rgb(0.10, 0.11, 0.13))),
                    ..container::Style::default()
                })
                .height(Fill),
            viewport,
        ]
        .into()
    }
    fn slice_current(&mut self) -> Task<Message> {
        self.settings.print_sequence = if self.by_object {
            String::from("by object")
        } else {
            String::from("by layer")
        };
        let settings = self.settings.clone();
        if let Some(model) = &self.model {
            let indices: Vec<usize> = model
                .plates
                .get(self.plate)
                .map(|p| p.object_indices.clone())
                .unwrap_or_else(|| (0..model.objects.len()).collect());
            if settings.print_sequence_by_object() && indices.len() > 1 {
                let mut results = Vec::new();
                for &i in &indices {
                    let Some(obj) = model.objects.get(i) else {
                        continue;
                    };
                    match slice_volumes_with_gpu_or_cpu(&obj.volumes_or_mesh(), &settings) {
                        Ok((result, _)) => results.push(result),
                        Err(err) => {
                            self.status = format!("slice failed: {err}");
                            return Task::none();
                        }
                    }
                }
                if let Err(err) = check_print_path_conflicts(&results) {
                    self.status = format!("slice failed: {err}");
                    return Task::none();
                }
                return self.finish_objects(&settings, results);
            }
            let vols = model.world_volumes_for_plate(self.plate);
            if vols.len() > 1
                || vols
                    .iter()
                    .any(bambu_model::ModelVolume::needs_volume_slice)
            {
                return match slice_volumes_with_gpu_or_cpu(&vols, &settings) {
                    Ok((result, backend)) => self.finish_slice(&settings, result, backend.as_str()),
                    Err(err) => {
                        self.status = format!("slice failed: {err}");
                        Task::none()
                    }
                };
            }
        }
        match slice_with_gpu_or_cpu(&self.scene.mesh, &settings) {
            Ok((result, backend)) => self.finish_slice(&settings, result, backend.as_str()),
            Err(err) => {
                self.status = format!("slice failed: {err}");
                Task::none()
            }
        }
    }

    fn finish_slice(
        &mut self,
        settings: &SliceSettings,
        result: bambu_slicer::SliceResult,
        backend: &str,
    ) -> Task<Message> {
        match write_gcode(settings, &result) {
            Ok(gcode) => {
                self.estimated_seconds = parse_gcode(&gcode).estimated_seconds;
                self.last_gcode = Some(gcode);
            }
            Err(err) => {
                self.status = format!("gcode failed: {err}");
                return Task::none();
            }
        }
        let support = result
            .layers
            .iter()
            .filter(|l| !l.support.is_empty() || !l.support_interface.is_empty())
            .count();
        self.status = format!(
            "sliced {} layers @ {}mm ({backend}; skirt {} · support {})",
            result.layers.len(),
            settings.layer_height_mm,
            result.layers.first().map(|l| l.skirt.len()).unwrap_or(0),
            support
        );
        self.scene
            .set_toolpaths(ToolpathBuffer::from_slice(&result));
        Task::none()
    }

    fn finish_objects(
        &mut self,
        settings: &SliceSettings,
        results: Vec<bambu_slicer::SliceResult>,
    ) -> Task<Message> {
        match write_gcode_for_objects(settings, &results) {
            Ok(gcode) => {
                self.estimated_seconds = parse_gcode(&gcode).estimated_seconds;
                self.last_gcode = Some(gcode);
            }
            Err(err) => {
                self.status = format!("gcode failed: {err}");
                return Task::none();
            }
        }
        let mut merged = bambu_slicer::SliceResult { layers: Vec::new() };
        for result in &results {
            merged.layers.extend(result.layers.iter().cloned());
        }
        self.status = format!(
            "sliced {} objects · {} layers (by object)",
            results.len(),
            merged.layers.len()
        );
        self.scene
            .set_toolpaths(ToolpathBuffer::from_slice(&merged));
        Task::none()
    }

    fn paint_pick(&mut self, ndc_x: f32, ndc_y: f32, aspect: f32) {
        let Some(kind) = self.paint_kind else {
            return;
        };
        let (origin, dir) = self.scene.camera.ray_from_ndc(ndc_x, ndc_y, aspect);
        let Some(hit) = self.scene.mesh.pick_triangle(origin, dir) else {
            self.status = "no triangle under cursor".into();
            return;
        };
        if self.model.is_none() {
            self.model = Some(Model::from_mesh("viewport", self.scene.mesh.clone()));
        }
        let idx = self.scene.mesh.indices[hit];
        let [a, b, c] = self.scene.mesh.triangle(idx);
        let centroid = (a + b + c) / 3.0;
        let tris = if self.brush_mm > 0.51 {
            let mut near = self.scene.mesh.triangles_near(centroid, self.brush_mm);
            if !near.contains(&hit) {
                near.push(hit);
            }
            near
        } else {
            vec![hit]
        };
        let paint = if self.paint_blocker {
            TrianglePaint::Blocker
        } else {
            TrianglePaint::Enforcer
        };
        let mut painted = 0usize;
        for tri in tris {
            if self.paint_one(kind, tri, paint) {
                painted += 1;
            }
        }
        self.refresh_paint_overlay();
        self.status = if painted > 0 {
            format!("painted {kind:?} x{painted} — Slice to apply")
        } else {
            format!("triangle {hit} not on a model volume")
        };
    }

    fn paint_one(&mut self, kind: PaintKind, tri: usize, paint: TrianglePaint) -> bool {
        let Some(model) = self.model.as_mut() else {
            return false;
        };
        let mut remaining = tri;
        for obj in &mut model.objects {
            if obj.volumes.is_empty() {
                obj.volumes = obj.volumes_or_mesh();
            }
            for vol in &mut obj.volumes {
                let n = vol.mesh.indices.len();
                if remaining >= n {
                    remaining -= n;
                    continue;
                }
                match kind {
                    PaintKind::Support => {
                        if vol.triangle_support.len() < n {
                            vol.triangle_support = vec![TrianglePaint::None; n];
                        }
                        vol.triangle_support[remaining] = paint;
                    }
                    PaintKind::Seam => {
                        if vol.triangle_seam.len() < n {
                            vol.triangle_seam = vec![TrianglePaint::None; n];
                        }
                        vol.triangle_seam[remaining] = paint;
                    }
                    PaintKind::Fuzzy => {
                        if vol.triangle_fuzzy_skin.len() < n {
                            vol.triangle_fuzzy_skin = vec![TrianglePaint::None; n];
                        }
                        vol.triangle_fuzzy_skin[remaining] = paint;
                    }
                }
                return true;
            }
        }
        false
    }

    fn plate_buttons(&self) -> Element<'_, Message> {
        let n = self
            .model
            .as_ref()
            .map(|m| m.plates.len().max(1))
            .unwrap_or(1);
        let mut r = row![];
        for i in 0..n {
            let name = self
                .model
                .as_ref()
                .and_then(|m| m.plates.get(i))
                .map(|p| p.name.clone())
                .filter(|s| !s.is_empty())
                .unwrap_or_else(|| format!("{}", i + 1));
            r = r.push(button(text(name).size(12)).on_press(Message::Plate(i)));
        }
        r.spacing(4).into()
    }

    fn object_panel(&self) -> Element<'_, Message> {
        let Some(model) = &self.model else {
            return text("cube").size(12).into();
        };
        if model.objects.is_empty() {
            return text("—").size(12).into();
        }
        let mut col = column![];
        for (i, obj) in model.objects.iter().enumerate() {
            let name = if obj.name.is_empty() {
                format!("object {}", i + 1)
            } else {
                obj.name.clone()
            };
            col = col.push(button(text(name).size(12)).on_press(Message::SelectObject(i)));
        }
        if let Some(obj) = model.objects.get(self.selected_object) {
            for (i, vol) in obj.volumes.iter().enumerate() {
                let extruder = vol
                    .config
                    .get("extruder")
                    .cloned()
                    .unwrap_or_else(|| "—".into());
                let label = format!(
                    "{} {} ext {extruder}{}",
                    vol.volume_type.as_str(),
                    if vol.name.is_empty() {
                        format!("v{}", i + 1)
                    } else {
                        vol.name.clone()
                    },
                    if vol.hidden { " (hidden)" } else { "" }
                );
                col = col.push(button(text(label).size(11)).on_press(Message::SelectVolume(i)));
            }
            if let Some(vol) = obj.volumes.get(self.selected_volume) {
                col = col.push(
                    checkbox(vol.hidden)
                        .label("Hide volume")
                        .on_toggle(Message::HideVolume),
                );
            }
        }
        col.spacing(4).into()
    }

    fn apply_named_profile(&mut self, kind: BblProfileKind, name: &str) {
        let list = match kind {
            BblProfileKind::Process => &self.process_profiles,
            BblProfileKind::Filament => &self.filament_profiles,
            BblProfileKind::Machine => &self.machine_profiles,
        };
        let Some(path) = list.iter().find(|p| p.name == name).map(|p| p.path.clone()) else {
            self.status = format!("no {name} profile");
            return;
        };
        match overlay_bbl_profile(&mut self.settings, &path) {
            Ok(()) => {
                match kind {
                    BblProfileKind::Process => self.process_name = Some(name.to_string()),
                    BblProfileKind::Filament => self.filament_name = Some(name.to_string()),
                    BblProfileKind::Machine => {
                        self.machine_name = Some(name.to_string());
                        self.scene.set_bed_mm(self.settings.bed_size_mm());
                    }
                }
                self.status = format!("overlay {}", path.display());
            }
            Err(err) => self.status = format!("profile: {err}"),
        }
    }

    fn selected_vol_mut(&mut self) -> Option<&mut bambu_model::ModelVolume> {
        let obj = self.model.as_mut()?.objects.get_mut(self.selected_object)?;
        if obj.volumes.is_empty() {
            obj.volumes = obj.volumes_or_mesh();
        }
        obj.volumes.get_mut(self.selected_volume)
    }

    fn sync_scene_mesh(&mut self) {
        let Some(mesh) = self
            .model
            .as_ref()
            .and_then(|m| m.mesh_for_plate(self.plate))
        else {
            return;
        };
        let keep = self.scene.keep_solid;
        self.scene.set_mesh(mesh);
        self.scene.keep_solid = keep;
        self.refresh_paint_overlay();
    }

    fn refresh_paint_overlay(&mut self) {
        self.scene.keep_solid = self.paint_kind.is_some() || !self.scene.paint_overlay.is_empty();
        let mut overlay = Vec::new();
        let Some(model) = &self.model else {
            self.scene.paint_overlay = overlay;
            return;
        };
        let mut offset = 0usize;
        for obj in &model.objects {
            for vol in &obj.volumes {
                let n = vol.mesh.indices.len();
                for field in [
                    &vol.triangle_support,
                    &vol.triangle_seam,
                    &vol.triangle_fuzzy_skin,
                ] {
                    for (i, paint) in field.iter().enumerate() {
                        let color = match paint {
                            TrianglePaint::Enforcer => Some(paint_overlay_color(true)),
                            TrianglePaint::Blocker => Some(paint_overlay_color(false)),
                            TrianglePaint::None => None,
                        };
                        if let Some(c) = color {
                            overlay.push((offset + i, c));
                        }
                    }
                }
                offset += n;
            }
        }
        self.scene.keep_solid = self.paint_kind.is_some() || !overlay.is_empty();
        self.scene.paint_overlay = overlay;
    }

    fn refresh_monitor(&self) -> Task<Message> {
        let host = self.host.clone();
        let code = self.access_code.clone();
        let serial = self.serial.clone();
        let send_via = self.send_via;
        Task::perform(
            async move {
                monitor::fetch_monitor(send_via, host, code, serial)
                    .await
                    .map(Box::new)
            },
            Message::Status,
        )
    }

    fn run_print_cmd(&self, cmd: PrintCmd) -> Task<Message> {
        let host = self.host.clone();
        let code = self.access_code.clone();
        let serial = self.serial.clone();
        let send_via = self.send_via;
        Task::perform(
            async move { monitor::run_cmd(send_via, host, code, serial, cmd).await },
            Message::PrintControl,
        )
    }
}

#[derive(Debug, Clone, Copy)]
enum PaintKind {
    Support,
    Seam,
    Fuzzy,
}

#[derive(Debug, Clone, Copy)]
enum PrintCmd {
    Pause,
    Resume,
    Stop,
    Speed(u8),
    Light(bool),
}

fn role_legend() -> String {
    format!(
        "{} · {} · {} · {} · {}",
        ExtrusionRole::OuterWall.label(),
        ExtrusionRole::InnerWall.label(),
        ExtrusionRole::Infill.label(),
        ExtrusionRole::Support.label(),
        ExtrusionRole::PrimeTower.label()
    )
}

fn format_eta(seconds: f64) -> String {
    let s = seconds.max(0.0) as u64;
    let h = s / 3600;
    let m = (s % 3600) / 60;
    let sec = s % 60;
    if h > 0 {
        format!("{h}h {m}m")
    } else {
        format!("{m}m {sec}s")
    }
}

fn calibration_block_path() -> Option<PathBuf> {
    let rel = PathBuf::from("tests/calibration_block/3D+Printer+Test.3mf");
    let mut candidates = vec![PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("../..")
        .join(&rel)];
    if let Ok(cwd) = std::env::current_dir() {
        candidates.push(cwd.join(&rel));
    }
    candidates.into_iter().find(|p| p.is_file())
}

fn lan_from(host: String, code: String, serial: String) -> LanBackend {
    let creds = bambu_protocol::load_from_dir(bambu_protocol::default_config_dir())
        .unwrap_or_else(|_| Default::default());
    LanBackend::new(host, code)
        .with_serial(serial)
        .with_credentials(creds)
}

fn default_slice_settings() -> SliceSettings {
    match bambu_config::bbl_oracle_paths() {
        Some(paths) => {
            let mut settings =
                load_bbl_process(&paths.process).unwrap_or_else(|_| SliceSettings::bbl_0_20());
            let _ = overlay_bbl_profile(&mut settings, &paths.machine);
            let _ = overlay_bbl_profile(&mut settings, &paths.filament);
            settings
        }
        None => SliceSettings::bbl_0_20(),
    }
}
