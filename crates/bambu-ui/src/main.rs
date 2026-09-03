#![forbid(unsafe_code)]

use std::process::Command;

use bambu_config::SliceSettings;
use bambu_gpu::{
    force_vulkan_env, probe_vulkan, slice_with_gpu_or_cpu, ToolpathBuffer, ViewportEvent,
    ViewportScene,
};
use bambu_io::load_stl;
use iced::widget::{button, column, container, row, shader, text};
use iced::{Color, Element, Fill, Theme};

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

    fn update(&mut self, message: Message) {
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
        }
    }

    fn view(&self) -> Element<'_, Message> {
        let sidebar = column![
            text("Bambu Studio").size(22),
            text("Rust rewrite · iced + wgpu").size(14),
            text(format!("GPU: {}", self.adapter)).size(13),
            button("Open STL").on_press(Message::OpenStl),
            button("Slice").on_press(Message::Slice),
            button("Reset camera").on_press(Message::ResetCamera),
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
