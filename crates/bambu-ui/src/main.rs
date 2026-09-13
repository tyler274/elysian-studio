#![forbid(unsafe_code)]

use bambu_alloc as _;
use std::process::Command;

use bambu_config::{load_bbl_process, overlay_bbl_profile, SliceSettings};
use bambu_device::{PrintJob, PrinterBackend};
use bambu_gcode::{write_gcode, write_gcode_for_objects};
use bambu_gpu::{
    force_vulkan_env, probe_vulkan, slice_volumes_with_gpu_or_cpu, slice_with_gpu_or_cpu,
    ExtrusionRole, ToolpathBuffer, ViewportEvent, ViewportScene,
};
use bambu_io::{load_mesh, load_model};
use bambu_model::{Model, TrianglePaint};
use bambu_protocol::{capture_chamber, ChamberCapture, LanBackend};
use bambu_slicer::check_print_path_conflicts;
use iced::widget::{
    button, checkbox, column, container, row, scrollable, shader, slider, text, text_input,
};
use iced::{Color, Element, Fill, Task, Theme};

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
    settings: SliceSettings,
    model: Option<Model>,
    plate: usize,
    by_object: bool,
    paint_kind: Option<PaintKind>,
    mqtt_status: String,
    ams_status: String,
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
    PreviewMove(u32),
    EnableSupport(bool),
    RefreshStatus,
    Status(Result<String, String>),
}

impl From<ViewportEvent> for Message {
    fn from(event: ViewportEvent) -> Self {
        Message::Viewport(event)
    }
}

impl App {
    fn new(adapter: String) -> Self {
        Self {
            adapter: adapter.clone(),
            scene: ViewportScene::with_cube(adapter),
            status: "20mm cube on 256mm bed".into(),
            host: String::new(),
            access_code: String::new(),
            serial: String::new(),
            last_gcode: None,
            settings: default_slice_settings(),
            model: None,
            plate: 0,
            by_object: false,
            paint_kind: None,
            mqtt_status: String::new(),
            ams_status: String::new(),
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
                self.scene.camera = bambu_gpu::OrbitCamera::looking_at_bed(bambu_gpu::BED_MM);
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
            }
            Message::HideInfill(v) => self.scene.hide_infill = v,
            Message::HideSupport(v) => self.scene.hide_support = v,
            Message::ByObject(v) => self.by_object = v,
            Message::PaintSupport => {
                self.paint_kind = Some(PaintKind::Support);
                self.status = "click a triangle to paint support".into();
            }
            Message::PaintSeam => {
                self.paint_kind = Some(PaintKind::Seam);
                self.status = "click a triangle to paint seam".into();
            }
            Message::PaintFuzzy => {
                self.paint_kind = Some(PaintKind::Fuzzy);
                self.status = "click a triangle to paint fuzzy".into();
            }
            Message::PaintClear => {
                self.paint_kind = None;
                self.status = "paint mode off".into();
            }
            Message::PreviewMove(i) => {
                self.scene.preview_vertices = i;
            }
            Message::EnableSupport(v) => self.settings.enable_support = v,
            Message::RefreshStatus => {
                if self.host.is_empty() || self.access_code.is_empty() {
                    self.status = "printer IP and LAN access code required".into();
                    return Task::none();
                }
                let host = self.host.clone();
                let code = self.access_code.clone();
                let serial = self.serial.clone();
                self.status = format!("MQTT status {host}…");
                return Task::perform(
                    async move {
                        let creds =
                            bambu_protocol::load_from_dir(bambu_protocol::default_config_dir())
                                .unwrap_or_else(|_| Default::default());
                        let backend = LanBackend::new(host, code)
                            .with_serial(serial)
                            .with_credentials(creds);
                        let st = backend.status().await.map_err(|e| e.to_string())?;
                        let ams = backend.ams().await.unwrap_or_default();
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
                        Ok(format!(
                            "nozzle {:.0}°C bed {:.0}°C · AMS {trays} · tray {:?}",
                            st.nozzle_temp_c, st.bed_temp_c, ams.active_slot
                        ))
                    },
                    Message::Status,
                );
            }
            Message::Status(Ok(msg)) => {
                self.mqtt_status = msg.clone();
                self.ams_status = msg.clone();
                self.status = msg;
            }
            Message::Status(Err(err)) => self.status = format!("status failed: {err}"),
        }
        Task::none()
    }

    fn view(&self) -> Element<'_, Message> {
        let max_layer = self.scene.toolpaths.layer_zs.len().saturating_sub(1) as f64;
        let max_move = self.scene.toolpaths.vertices.len().saturating_sub(1) as f64;
        let plate_row = self.plate_buttons();
        let object_list = self.object_labels();
        let sidebar = scrollable(
            column![
                text("Bambu Studio").size(22),
                text("Rust rewrite · iced + wgpu").size(14),
                text(format!("GPU: {}", self.adapter)).size(13),
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
                text(object_list).size(12),
                text("Preview / G-code scrubber").size(16),
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
                button("Paint support").on_press(Message::PaintSupport),
                button("Paint seam").on_press(Message::PaintSeam),
                button("Paint fuzzy").on_press(Message::PaintFuzzy),
                button("Paint off").on_press(Message::PaintClear),
                button("Open model").on_press(Message::OpenModel),
                button("Slice").on_press(Message::Slice),
                button("Reset camera").on_press(Message::ResetCamera),
                button("Extract keys").on_press(Message::ExtractKeys),
                button("Discover printers").on_press(Message::Discover),
                text_input("printer IP", &self.host).on_input(Message::Host),
                text_input("LAN access code", &self.access_code)
                    .secure(true)
                    .on_input(Message::AccessCode),
                text_input("serial (optional)", &self.serial).on_input(Message::Serial),
                button("MQTT / AMS status").on_press(Message::RefreshStatus),
                text(if self.mqtt_status.is_empty() {
                    "AMS: —"
                } else {
                    self.mqtt_status.as_str()
                })
                .size(12),
                button("Send last slice").on_press(Message::Send),
                button("Chamber / RTSPS live").on_press(Message::Chamber),
                text(&self.status).size(13),
                text("Drag: orbit · Scroll: zoom · Click: paint").size(12),
            ]
            .spacing(8)
            .padding(16)
            .width(300),
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
            Ok(gcode) => self.last_gcode = Some(gcode),
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
            Ok(gcode) => self.last_gcode = Some(gcode),
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
        let Some(tri) = self.scene.mesh.pick_triangle(origin, dir) else {
            self.status = "no triangle under cursor".into();
            return;
        };
        if self.model.is_none() {
            self.model = Some(Model::from_mesh("viewport", self.scene.mesh.clone()));
        }
        let Some(model) = self.model.as_mut() else {
            return;
        };
        let paint = TrianglePaint::Enforcer;
        let mut remaining = tri;
        let mut painted = false;
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
                painted = true;
                break;
            }
            if painted {
                break;
            }
        }
        self.status = if painted {
            format!("painted {kind:?} triangle {tri} — Slice to apply")
        } else {
            format!("triangle {tri} not on a model volume")
        };
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

    fn object_labels(&self) -> String {
        let Some(model) = &self.model else {
            return "Objects: cube".into();
        };
        if model.objects.is_empty() {
            return "Objects: —".into();
        }
        let names: Vec<String> = model
            .objects
            .iter()
            .map(|o| {
                if o.name.is_empty() {
                    "object".into()
                } else {
                    o.name.clone()
                }
            })
            .collect();
        format!("Objects: {}", names.join(", "))
    }
}

#[derive(Debug, Clone, Copy)]
enum PaintKind {
    Support,
    Seam,
    Fuzzy,
}

fn role_legend() -> String {
    format!(
        "Roles: {:?} {:?} {:?} {:?} {:?}",
        ExtrusionRole::OuterWall,
        ExtrusionRole::InnerWall,
        ExtrusionRole::Infill,
        ExtrusionRole::Support,
        ExtrusionRole::PrimeTower
    )
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
