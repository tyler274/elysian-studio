#![forbid(unsafe_code)]

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
    capture_chamber, describe_hms, load_cached_catalog, refresh_catalog, ChamberCapture,
    CloudBackend, LanBackend,
};
use bambu_slicer::check_print_path_conflicts;
use iced::widget::{
    button, checkbox, column, container, pick_list, row, scrollable, shader, slider, text,
    text_input,
};
use iced::{Color, Element, Fill, Subscription, Task, Theme};

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
    use_cloud: bool,
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
    ChamberShot(Result<String, String>),
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
    UseCloud(bool),
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

impl App {
    fn new(adapter: String) -> Self {
        let settings = default_slice_settings();
        let bed = settings.bed_size_mm();
        let scene = ViewportScene::with_cube_on_bed(adapter.clone(), bed);
        Self {
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
            use_cloud: false,
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
                if self.host.is_empty() || self.access_code.is_empty() {
                    self.status = "printer IP and LAN access code required".into();
                    return Task::none();
                }
                let host = self.host.clone();
                let code = self.access_code.clone();
                let serial = self.serial.clone();
                let mapping = self.settings.filament_map.clone();
                self.status = format!("FTPS + MQTT to {host}…");
                return Task::perform(
                    async move {
                        let creds =
                            bambu_protocol::load_from_dir(bambu_protocol::default_config_dir())
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
                        std::thread::spawn(move || match capture_chamber(&host, &code) {
                            Ok(ChamberCapture::Jpeg(jpeg)) => {
                                let frame = bambu_protocol::jpeg_to_frame(&jpeg)
                                    .map_err(|err| err.to_string())?;
                                Ok(format!(
                                    "chamber JPEG {}×{} ({} bytes)",
                                    frame.width,
                                    frame.height,
                                    jpeg.len()
                                ))
                            }
                            Ok(ChamberCapture::Rtsps { url, options }) => {
                                let first = options.lines().next().unwrap_or("RTSPS");
                                Ok(format!("chamber RTSPS {url} · {first}"))
                            }
                            Err(err) => Err(err.to_string()),
                        })
                        .join()
                        .unwrap_or_else(|_| Err("camera thread panicked".into()))
                    },
                    Message::ChamberShot,
                );
            }
            Message::ChamberShot(Ok(msg)) => self.status = msg,
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
            Message::UseCloud(v) => self.use_cloud = v,
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
        let max_layer = self.scene.toolpaths.layer_zs.len().saturating_sub(1) as f64;
        let max_move = self.scene.toolpaths.vertices.len().saturating_sub(1) as f64;
        let plate_row = self.plate_buttons();
        let process_names: Vec<String> = self
            .process_profiles
            .iter()
            .map(|p| p.name.clone())
            .collect();
        let filament_names: Vec<String> = self
            .filament_profiles
            .iter()
            .map(|p| p.name.clone())
            .collect();
        let machine_names: Vec<String> = self
            .machine_profiles
            .iter()
            .map(|p| p.name.clone())
            .collect();
        let preview_z = self.scene.preview_z();
        let layer_frac = if max_layer <= 0.0 {
            1.0
        } else {
            f64::from(self.scene.preview_layer) / max_layer
        };
        let eta = self
            .estimated_seconds
            .map(|s| format_eta(s * layer_frac))
            .unwrap_or_else(|| "—".into());
        let sidebar = scrollable(
            column![
                text("Bambu Studio").size(22),
                text("Rust rewrite · iced + wgpu").size(14),
                text(format!("GPU: {}", self.adapter)).size(13),
                text("Profiles").size(16),
                pick_list(
                    process_names,
                    self.process_name.clone(),
                    Message::ProcessProfile
                )
                .placeholder("process JSON"),
                pick_list(
                    filament_names,
                    self.filament_name.clone(),
                    Message::FilamentProfile
                )
                .placeholder("filament JSON"),
                pick_list(
                    machine_names,
                    self.machine_name.clone(),
                    Message::MachineProfile
                )
                .placeholder("machine JSON"),
                text(format!("Bed {:.0} mm", self.scene.bed_mm)).size(12),
                text("Process").size(16),
                row![
                    button("-").on_press(Message::WallLoops(
                        self.settings.wall_loops.saturating_sub(1).max(1)
                    )),
                    text(format!("Walls {}", self.settings.wall_loops)).size(13),
                    button("+")
                        .on_press(Message::WallLoops((self.settings.wall_loops + 1).min(10))),
                ]
                .spacing(6),
                text(format!("Layer {:.2} mm", self.settings.layer_height_mm)).size(13),
                slider(
                    0.08..=0.32,
                    self.settings.layer_height_mm,
                    Message::LayerHeight
                )
                .step(0.01),
                text(format!(
                    "Infill {:.0}% {}",
                    self.settings.infill_density * 100.0,
                    self.settings.infill_pattern.as_str()
                ))
                .size(13),
                slider(0.0..=1.0, self.settings.infill_density, Message::Infill).step(0.01),
                checkbox(self.settings.enable_support)
                    .label("Supports")
                    .on_toggle(Message::EnableSupport),
                text("Filament").size(16),
                text(format!(
                    "{} {} · {}°C · {} tool(s)",
                    self.settings.filament_vendor,
                    self.settings.filament_type,
                    self.settings.temperature_c,
                    self.settings.filament_count.max(1)
                ))
                .size(13),
                text(format!("Nozzle {}°C", self.settings.temperature_c)).size(13),
                slider(
                    180.0..=280.0,
                    f64::from(self.settings.temperature_c),
                    Message::NozzleTemp,
                )
                .step(1.0),
                text("Printer").size(16),
                text(format!(
                    "{} · {}",
                    self.settings.printer_structure, self.settings.curr_bed_type
                ))
                .size(13),
                checkbox(self.by_object)
                    .label("By-object sequence")
                    .on_toggle(Message::ByObject),
                text("Plates").size(16),
                plate_row,
                text("Objects").size(16),
                self.object_panel(),
                text("Preview / G-code scrubber").size(16),
                text(format!(
                    "Layer {}  Z {:.2} mm  ~{eta}",
                    self.scene.preview_layer + 1,
                    if preview_z.is_finite() {
                        preview_z
                    } else {
                        0.0
                    }
                ))
                .size(12),
                slider(
                    0.0..=max_layer.max(1.0),
                    f64::from(self.scene.preview_layer),
                    |v| { Message::PreviewLayer(v as u32) }
                )
                .step(1.0),
                slider(
                    0.0..=max_move.max(1.0),
                    f64::from(self.scene.preview_vertices),
                    |v| Message::PreviewMove(v as u32)
                )
                .step(1.0),
                text(role_legend()).size(11),
                checkbox(self.scene.hide_infill)
                    .label("Hide infill")
                    .on_toggle(Message::HideInfill),
                checkbox(self.scene.hide_support)
                    .label("Hide support")
                    .on_toggle(Message::HideSupport),
                text("Paint (click triangle)").size(16),
                checkbox(self.paint_blocker)
                    .label("Blocker (else Enforcer)")
                    .on_toggle(Message::PaintBlocker),
                text(format!("Brush {:.1} mm", self.brush_mm)).size(12),
                slider(0.5..=12.0, f64::from(self.brush_mm), |v| {
                    Message::BrushRadius(v as f32)
                })
                .step(0.5),
                button("Paint support").on_press(Message::PaintSupport),
                button("Paint seam").on_press(Message::PaintSeam),
                button("Paint fuzzy").on_press(Message::PaintFuzzy),
                button("Paint off").on_press(Message::PaintClear),
                button("Open model").on_press(Message::OpenModel),
                button("Calibration block").on_press(Message::Calibration),
                button("Slice").on_press(Message::Slice),
                button("Reset camera").on_press(Message::ResetCamera),
                button("Extract keys").on_press(Message::ExtractKeys),
                button("Discover printers").on_press(Message::Discover),
                text_input("printer IP", &self.host).on_input(Message::Host),
                text_input("LAN access code", &self.access_code)
                    .secure(true)
                    .on_input(Message::AccessCode),
                text_input("serial (optional)", &self.serial).on_input(Message::Serial),
                checkbox(self.use_cloud)
                    .label("Cloud MQTT (token in config dir)")
                    .on_toggle(Message::UseCloud),
                checkbox(self.live_monitor)
                    .label("Live monitor")
                    .on_toggle(Message::LiveMonitor),
                button("MQTT / AMS status").on_press(Message::RefreshStatus),
                text(self.monitor_line()).size(12),
                self.ams_chips(),
                row![
                    button("Pause").on_press(Message::Pause),
                    button("Resume").on_press(Message::Resume),
                    button("Stop").on_press(Message::Stop),
                ]
                .spacing(6),
                text("HMS").size(16),
                button("Refresh HMS catalog").on_press(Message::RefreshHms),
                text(if self.hms_lines.is_empty() {
                    "no HMS".into()
                } else {
                    self.hms_lines.join("\n")
                })
                .size(11),
                button("Send last slice").on_press(Message::Send),
                button("Chamber / RTSPS live").on_press(Message::Chamber),
                text(&self.status).size(13),
                text("Drag: orbit · Scroll: zoom · Click: paint").size(12),
            ]
            .spacing(8)
            .padding(16)
            .width(320),
        )
        .height(Fill);

        let viewport = shader(&self.scene).width(Fill).height(Fill);

        row![
            container(sidebar)
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

    fn ams_chips(&self) -> Element<'_, Message> {
        let mut r = row![];
        if self.settings.filament_map.is_empty() {
            r = r.push(text("AMS map: —").size(11));
        }
        for (i, mapped) in self.settings.filament_map.iter().enumerate() {
            r = r.push(
                button(text(format!("F{}→T{mapped}", i + 1)).size(11))
                    .on_press(Message::CycleAmsMap(i)),
            );
        }
        r.spacing(4).into()
    }

    fn monitor_line(&self) -> String {
        if self.mqtt_status.is_empty() {
            return "AMS: —".into();
        }
        let humidity = self
            .ams
            .humidity
            .map(|h| format!(" RH{h}"))
            .unwrap_or_default();
        format!(
            "{} · {}% · L{}/{} · {}m · nozzle {:.0}°C{humidity}",
            if self.machine.gcode_state.is_empty() {
                "—"
            } else {
                self.machine.gcode_state.as_str()
            },
            self.machine.mc_percent,
            self.machine.layer_num,
            self.machine.total_layer_num,
            self.machine.mc_remaining_time_min,
            self.machine.nozzle_temp_c
        )
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
        let use_cloud = self.use_cloud;
        Task::perform(
            async move {
                fetch_monitor(use_cloud, host, code, serial)
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
        let use_cloud = self.use_cloud;
        Task::perform(
            async move { run_cmd(use_cloud, host, code, serial, cmd).await },
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

async fn snapshot_backend<B: PrinterBackend>(backend: B) -> Result<MonitorSnapshot, String> {
    let st = backend.status().await.map_err(|e| e.to_string())?;
    let ams = backend.ams().await.unwrap_or_default();
    let catalog = load_cached_catalog(bambu_protocol::default_config_dir(), "en");
    let hms_lines = st
        .hms
        .iter()
        .map(|h| describe_hms(catalog.as_ref(), *h, "en"))
        .collect();
    let trays = if ams.trays.is_empty() {
        format!("{} slots", ams.slot_count)
    } else {
        ams.trays
            .iter()
            .map(|t| {
                format!(
                    "T{} {} {}",
                    t.id,
                    t.filament_type,
                    if t.color.is_empty() { "—" } else { &t.color }
                )
            })
            .collect::<Vec<_>>()
            .join(" · ")
    };
    Ok(MonitorSnapshot {
        line: format!(
            "{} {}% L{}/{} nozzle {:.0}°C · AMS {trays}",
            st.gcode_state, st.mc_percent, st.layer_num, st.total_layer_num, st.nozzle_temp_c
        ),
        machine: st,
        ams,
        hms_lines,
    })
}

async fn fetch_monitor(
    use_cloud: bool,
    host: String,
    code: String,
    serial: String,
) -> Result<MonitorSnapshot, String> {
    if use_cloud {
        let backend = CloudBackend::from_config_dir(bambu_protocol::default_config_dir())
            .map_err(|e| e.to_string())?;
        return snapshot_backend(backend).await;
    }
    if host.is_empty() || code.is_empty() {
        return Err("printer IP and LAN access code required".into());
    }
    snapshot_backend(lan_from(host, code, serial)).await
}

async fn run_cmd(
    use_cloud: bool,
    host: String,
    code: String,
    serial: String,
    cmd: PrintCmd,
) -> Result<String, String> {
    async fn go<B: PrinterBackend>(backend: B, cmd: PrintCmd) -> Result<String, String> {
        match cmd {
            PrintCmd::Pause => backend.pause().await,
            PrintCmd::Resume => backend.resume().await,
            PrintCmd::Stop => backend.stop().await,
        }
        .map_err(|e| e.to_string())?;
        Ok(match cmd {
            PrintCmd::Pause => "pause sent",
            PrintCmd::Resume => "resume sent",
            PrintCmd::Stop => "stop sent",
        }
        .into())
    }
    if use_cloud {
        let backend = CloudBackend::from_config_dir(bambu_protocol::default_config_dir())
            .map_err(|e| e.to_string())?;
        return go(backend, cmd).await;
    }
    if host.is_empty() || code.is_empty() {
        return Err("printer IP and LAN access code required".into());
    }
    go(lan_from(host, code, serial), cmd).await
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
