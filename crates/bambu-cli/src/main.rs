#![forbid(unsafe_code)]

use std::path::PathBuf;

use bambu_config::{
    bbl_oracle_paths, load_bbl_process, ConfigError, InfillPattern, IroningType, SeamPosition,
    SliceSettings,
};
use bambu_device::{PrintJob, PrinterBackend};
use bambu_gcode::write_gcode;
use bambu_gpu::{slice_on_vulkan, slice_with_gpu_or_cpu, SliceBackend};
use bambu_io::load_stl;
use bambu_protocol::{
    default_config_dir, install_app_cert, load_from_dir, send_gcode_line, snapshot_jpeg, LanBackend,
};
use bambu_slicer::slice_mesh;
use clap::{Parser, Subcommand};
use thiserror::Error;

#[derive(Debug, Error)]
pub enum CliError {
    #[error(transparent)]
    Io(#[from] bambu_io::IoError),
    #[error(transparent)]
    Slice(#[from] bambu_slicer::SlicerError),
    #[error(transparent)]
    Gcode(#[from] bambu_gcode::GcodeError),
    #[error(transparent)]
    Gpu(#[from] bambu_gpu::GpuError),
    #[error(transparent)]
    StdIo(#[from] std::io::Error),
    #[error(transparent)]
    Config(#[from] ConfigError),
    #[error("unknown infill pattern '{0}' (rectilinear|grid|concentric|gyroid|honeycomb)")]
    InfillPattern(String),
    #[error("unknown seam '{0}' (aligned|rear|nearest|random)")]
    Seam(String),
    #[error("unknown ironing '{0}' (off|top|topmost|solid)")]
    Ironing(String),
    #[error("{0}")]
    Message(String),
}

#[derive(Parser)]
#[command(
    name = "bambu-cli",
    about = "Headless Bambu Studio slicer (Rust rewrite)"
)]
struct Cli {
    #[command(subcommand)]
    command: Commands,
}

#[derive(Subcommand)]
enum Commands {
    /// Slice an STL to G-code (Vulkan plane intersection when available).
    Slice {
        input: PathBuf,
        #[arg(short, long)]
        output: PathBuf,
        /// Bambu process JSON (follows `inherits` in the same directory).
        #[arg(long)]
        settings: Option<PathBuf>,
        /// Load upstream `0.20mm Standard @BBL X1C` (15% grid, brim 5, 5 top shells).
        #[arg(long)]
        bbl_0_20: bool,
        #[arg(long)]
        layer_height: Option<f64>,
        #[arg(long)]
        infill: Option<f64>,
        #[arg(long)]
        walls: Option<u32>,
        #[arg(long)]
        infill_pattern: Option<String>,
        #[arg(long)]
        seam: Option<String>,
        /// Skirt loops around the first layer (0 disables).
        #[arg(long)]
        skirt: Option<u32>,
        /// Gap from object/brim to the innermost skirt loop, millimeters.
        #[arg(long)]
        skirt_distance: Option<f64>,
        /// Outer brim width in millimeters (0 disables).
        #[arg(long)]
        brim: Option<f64>,
        /// Generate classic grid supports under overhangs.
        #[arg(long)]
        support: bool,
        /// Overhang threshold from vertical, degrees.
        #[arg(long)]
        support_angle: Option<f64>,
        /// Solid bottom shell layers.
        #[arg(long)]
        bottom: Option<u32>,
        /// Solid top shell layers.
        #[arg(long)]
        top: Option<u32>,
        /// Ironing: off, top, topmost, or solid.
        #[arg(long)]
        ironing: Option<String>,
        /// Force CPU triangle–plane intersection.
        #[arg(long, conflicts_with = "gpu")]
        cpu: bool,
        /// Require Vulkan compute; error if the adapter is missing.
        #[arg(long, conflicts_with = "cpu")]
        gpu: bool,
    },
    /// Option B slicer credentials (extract / import / status).
    Keys {
        #[command(subcommand)]
        command: KeysCommand,
    },
    /// LAN printer discovery (SSDP UDP 2021).
    Device {
        #[command(subcommand)]
        command: DeviceCommand,
    },
}

#[derive(Subcommand)]
enum KeysCommand {
    /// Scan a local stock plugin / known config dirs and write slicer_*.pem.
    Extract {
        /// Path to libbambu_networking.so / bambu_networking.dll.
        #[arg(long)]
        plugin: Option<PathBuf>,
        /// Destination directory (default: $XDG_CONFIG_HOME/bambu-studio-rs).
        #[arg(long)]
        out: Option<PathBuf>,
    },
    /// Show whether Option B PEMs are present.
    Status {
        #[arg(long)]
        dir: Option<PathBuf>,
    },
}

#[derive(Subcommand)]
enum DeviceCommand {
    /// Listen for Bambu SSDP advertisements.
    Discover {
        #[arg(long, default_value_t = 3)]
        timeout: u64,
    },
    /// MQTT `pushall` and print nozzle/bed temps.
    Status {
        #[arg(long)]
        host: String,
        /// LAN access code (printer screen).
        #[arg(long)]
        code: String,
        /// Serial / USN. Omit to read the MQTT certificate CN.
        #[arg(long, default_value = "")]
        serial: String,
    },
    /// FTPS-upload a G-code file as `.gcode.3mf` and MQTT `project_file`.
    Send {
        file: PathBuf,
        #[arg(long)]
        host: String,
        #[arg(long)]
        code: String,
        #[arg(long, default_value = "")]
        serial: String,
        /// Remote basename (default: input stem).
        #[arg(long)]
        name: Option<String>,
    },
    /// MQTT `gcode_line` (Developer Mode or signed Option B).
    Gcode {
        #[arg(long)]
        host: String,
        #[arg(long)]
        code: String,
        #[arg(long, default_value = "")]
        serial: String,
        #[arg(long)]
        line: String,
    },
    /// MQTT `security.app_cert_install` (needs slicer_cert.pem + slicer_crl.pem).
    InstallCert {
        #[arg(long)]
        host: String,
        #[arg(long)]
        code: String,
        #[arg(long, default_value = "")]
        serial: String,
    },
    /// Grab one P1/A1 chamber JPEG (TLS :6000). X1/H2 use RTSPS :322 instead.
    Camera {
        #[arg(long)]
        host: String,
        #[arg(long)]
        code: String,
        #[arg(long)]
        output: PathBuf,
    },
}

fn main() {
    tracing_subscriber::fmt()
        .with_env_filter(
            tracing_subscriber::EnvFilter::from_default_env()
                .add_directive("bambu_cli=info".parse().unwrap()),
        )
        .init();

    if let Err(err) = run() {
        eprintln!("error: {err}");
        std::process::exit(1);
    }
}

fn run() -> Result<(), CliError> {
    let cli = Cli::parse();
    match cli.command {
        Commands::Slice {
            input,
            output,
            settings,
            bbl_0_20,
            layer_height,
            infill,
            walls,
            infill_pattern,
            seam,
            skirt,
            skirt_distance,
            brim,
            support,
            support_angle,
            bottom,
            top,
            ironing,
            cpu,
            gpu,
        } => {
            let mut slice_settings = if let Some(path) = settings {
                load_bbl_process(path)?
            } else if bbl_0_20 {
                if let Some(paths) = bbl_oracle_paths() {
                    load_bbl_process(&paths.process)?
                } else {
                    SliceSettings::bbl_0_20()
                }
            } else {
                SliceSettings::default()
            };
            if let Some(h) = layer_height {
                slice_settings.layer_height_mm = h;
            }
            if let Some(i) = infill {
                slice_settings.infill_density = i;
            }
            if let Some(w) = walls {
                slice_settings.wall_loops = w.max(1);
            }
            if let Some(name) = infill_pattern {
                slice_settings.infill_pattern =
                    InfillPattern::from_name(&name).ok_or(CliError::InfillPattern(name))?;
            }
            if let Some(name) = seam {
                slice_settings.seam = SeamPosition::from_name(&name).ok_or(CliError::Seam(name))?;
            }
            if let Some(s) = skirt {
                slice_settings.skirt_loops = s;
            }
            if let Some(d) = skirt_distance {
                slice_settings.skirt_distance_mm = d;
            }
            if let Some(b) = brim {
                slice_settings.brim_width_mm = b.max(0.0);
            }
            if support {
                slice_settings.enable_support = true;
            }
            if let Some(a) = support_angle {
                slice_settings.support_threshold_angle_deg = a.clamp(0.0, 89.0);
            }
            if let Some(n) = bottom {
                slice_settings.bottom_shell_layers = n;
            }
            if let Some(n) = top {
                slice_settings.top_shell_layers = n;
            }
            if let Some(name) = ironing {
                slice_settings.ironing_type =
                    IroningType::from_name(&name).ok_or(CliError::Ironing(name))?;
            }
            let gcode = slice_file(&input, &slice_settings, cpu, gpu)?;
            std::fs::write(&output, gcode)?;
            tracing::info!("wrote {}", output.display());
        }
        Commands::Keys { command } => match command {
            KeysCommand::Extract { plugin, out } => {
                let report =
                    bambu_protocol::extract_to_config_dir(plugin.as_deref(), out.as_deref())
                        .map_err(|err| CliError::Message(err.to_string()))?;
                for note in &report.notes {
                    println!("{note}");
                }
                for line in report.credentials.status_lines() {
                    println!("{line}");
                }
            }
            KeysCommand::Status { dir } => {
                let dir = dir.unwrap_or_else(bambu_protocol::default_config_dir);
                let creds = bambu_protocol::load_from_dir(&dir)
                    .map_err(|err| CliError::Message(err.to_string()))?;
                println!("config dir: {}", dir.display());
                for line in creds.status_lines() {
                    println!("{line}");
                }
            }
        },
        Commands::Device { command } => match command {
            DeviceCommand::Discover { timeout } => {
                let printers = bambu_protocol::discover(std::time::Duration::from_secs(timeout))
                    .map_err(|err| CliError::Message(err.to_string()))?;
                if printers.is_empty() {
                    println!("no printers advertised on UDP 2021 ({timeout}s)");
                } else {
                    for p in printers {
                        println!(
                            "{}  {}  {}  ({})",
                            p.dev_id, p.dev_ip, p.dev_name, p.dev_type
                        );
                    }
                }
            }
            DeviceCommand::Status { host, code, serial } => {
                let backend = lan_backend(host, code, serial)?;
                let st = block_on(backend.status())?;
                println!(
                    "{}  {}  nozzle={:.1}C  bed={:.1}C  online={}  developer_mode={}",
                    st.serial,
                    st.name,
                    st.nozzle_temp_c,
                    st.bed_temp_c,
                    st.online,
                    st.developer_mode
                );
            }
            DeviceCommand::Send {
                file,
                host,
                code,
                serial,
                name,
            } => {
                let gcode = std::fs::read_to_string(&file)?;
                let filename = name.unwrap_or_else(|| {
                    file.file_name()
                        .and_then(|n| n.to_str())
                        .unwrap_or("job.gcode")
                        .to_string()
                });
                let backend = lan_backend(host, code, serial)?;
                block_on(backend.start_print(PrintJob { filename, gcode }))?;
                println!("print command sent");
            }
            DeviceCommand::Gcode {
                host,
                code,
                serial,
                line,
            } => {
                let backend = lan_backend(host, code, serial)?;
                let report = tokio::runtime::Runtime::new()?
                    .block_on(send_gcode_line(&backend, &line))
                    .map_err(|err| CliError::Message(err.to_string()))?;
                match report {
                    Some(body) => println!("{body}"),
                    None => println!("gcode_line published (no report within 5s)"),
                }
            }
            DeviceCommand::InstallCert { host, code, serial } => {
                let backend = lan_backend(host, code, serial)?;
                let report = tokio::runtime::Runtime::new()?
                    .block_on(install_app_cert(&backend))
                    .map_err(|err| CliError::Message(err.to_string()))?;
                match report {
                    Some(body) => println!("{body}"),
                    None => println!("app_cert_install published (no report within 8s)"),
                }
            }
            DeviceCommand::Camera { host, code, output } => {
                let jpeg = snapshot_jpeg(&host, &code)
                    .map_err(|err| CliError::Message(err.to_string()))?;
                std::fs::write(&output, &jpeg)?;
                println!("wrote {} ({} bytes)", output.display(), jpeg.len());
            }
        },
    }
    Ok(())
}

pub fn slice_file(
    input: &std::path::Path,
    settings: &SliceSettings,
    force_cpu: bool,
    force_gpu: bool,
) -> Result<String, CliError> {
    let mesh = load_stl(input)?;
    let (sliced, backend) = if force_cpu {
        (slice_mesh(&mesh, settings)?, SliceBackend::Cpu)
    } else if force_gpu {
        (
            slice_on_vulkan(&mesh, settings)?,
            SliceBackend::VulkanCompute,
        )
    } else {
        slice_with_gpu_or_cpu(&mesh, settings)?
    };
    tracing::info!("sliced {} layers ({backend})", sliced.layers.len());
    Ok(write_gcode(settings, &sliced)?)
}

fn lan_backend(host: String, code: String, serial: String) -> Result<LanBackend, CliError> {
    let creds = load_from_dir(default_config_dir()).unwrap_or_else(|_| Default::default());
    Ok(LanBackend::new(host, code)
        .with_serial(serial)
        .with_credentials(creds))
}

fn block_on<T>(
    fut: impl std::future::Future<Output = Result<T, bambu_device::DeviceError>>,
) -> Result<T, CliError> {
    tokio::runtime::Runtime::new()?
        .block_on(fut)
        .map_err(|err| CliError::Message(err.to_string()))
}
