#![forbid(unsafe_code)]

use std::path::PathBuf;

use bambu_config::{
    bbl_oracle_paths, load_bbl_process, ConfigError, InfillPattern, SeamPosition, SliceSettings,
};
use bambu_gcode::write_gcode;
use bambu_gpu::{slice_on_vulkan, slice_with_gpu_or_cpu, SliceBackend};
use bambu_io::load_stl;
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
}

#[derive(Parser)]
#[command(name = "bambu-cli", about = "Headless Bambu Studio slicer (Rust rewrite)")]
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
        /// Force CPU triangle–plane intersection.
        #[arg(long, conflicts_with = "gpu")]
        cpu: bool,
        /// Require Vulkan compute; error if the adapter is missing.
        #[arg(long, conflicts_with = "cpu")]
        gpu: bool,
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
            let gcode = slice_file(&input, &slice_settings, cpu, gpu)?;
            std::fs::write(&output, gcode)?;
            tracing::info!("wrote {}", output.display());
        }
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
