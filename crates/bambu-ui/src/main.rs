#![forbid(unsafe_code)]

use std::process::Command;

use bambu_config::SliceSettings;
use bambu_gpu::{
    force_vulkan_env, probe_vulkan, slice_with_gpu_or_cpu, ToolpathBuffer, ViewportEvent,
    ViewportScene,
};
use bambu_io::load_stl;
use iced::widget::{button, column, container, row, shader, text};
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
}

#[derive(Debug, Clone)]
enum Message {
    Viewport(ViewportEvent),
    OpenStl,
    Slice,
    ResetCamera,
    ExtractKeys,
    Discover,
    Discovered(Result<Vec<bambu_protocol::DiscoveredPrinter>, String>),
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
            Message::OpenStl => {
                if let Some(path) = rfd::FileDialog::new()
                    .add_filter("STL", &["stl", "STL"])
                    .pick_file()
                {
                    match load_stl(&path) {
                        Ok(mesh) => {
                            let tris = mesh.indices.len();
                            self.scene.set_mesh(mesh);
                            self.status = format!(
                                "loaded {} ({} triangles)",
                                path.file_name()
                                    .and_then(|n| n.to_str())
                                    .unwrap_or("mesh"),
                                tris
                            );
                        }
                        Err(err) => self.status = format!("open failed: {err}"),
                    }
                }
            }
            Message::Slice => {
                let settings = SliceSettings::default();
                match slice_with_gpu_or_cpu(&self.scene.mesh, &settings) {
                    Ok((result, backend)) => {
                        self.scene
                            .set_toolpaths(ToolpathBuffer::from_slice(&result));
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
                    }
                    Err(err) => self.status = format!("slice failed: {err}"),
                }
            }
            Message::ResetCamera => {
                self.scene.camera = bambu_gpu::OrbitCamera::looking_at_bed(bambu_gpu::BED_MM);
            }
            Message::ExtractKeys => {
                match bambu_protocol::extract_to_config_dir(None, None) {
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
                }
            }
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
                self.status = list
                    .iter()
                    .map(|p| format!("{} {}", p.dev_ip, p.dev_name))
                    .collect::<Vec<_>>()
                    .join(" · ");
            }
            Message::Discovered(Err(err)) => {
                self.status = format!("discover failed: {err}");
            }
        }
        Task::none()
    }

    fn view(&self) -> Element<'_, Message> {
        let sidebar = column![
            text("Bambu Studio").size(22),
            text("Rust rewrite · iced + wgpu").size(14),
            text(format!("GPU: {}", self.adapter)).size(13),
            button("Open STL").on_press(Message::OpenStl),
            button("Slice").on_press(Message::Slice),
            button("Reset camera").on_press(Message::ResetCamera),
            button("Extract keys").on_press(Message::ExtractKeys),
            button("Discover printers").on_press(Message::Discover),
            text(&self.status).size(13),
            text("Drag: orbit · Scroll: zoom").size(12),
        ]
        .spacing(8)
        .padding(16)
        .width(280);

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
}
