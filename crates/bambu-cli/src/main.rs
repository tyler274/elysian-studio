#![forbid(unsafe_code)]

use std::path::PathBuf;

use bambu_config::{InfillPattern, SeamPosition, SliceSettings};
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
        #[arg(long, default_value_t = 0.2)]
        layer_height: f64,
        #[arg(long, default_value_t = 0.20)]
        infill: f64,
        #[arg(long, default_value_t = 2)]
        walls: u32,
        #[arg(long, default_value = "gyroid")]
        infill_pattern: String,
        #[arg(long, default_value = "aligned")]
        seam: String,
        /// Skirt loops around the first layer (0 disables).
        #[arg(long, default_value_t = 2)]
        skirt: u32,
        /// Gap from object/brim to the innermost skirt loop, millimeters.
        #[arg(long, default_value_t = 2.0)]
        skirt_distance: f64,
        /// Outer brim width in millimeters (0 disables).
        #[arg(long, default_value_t = 0.0)]
        brim: f64,
        /// Generate classic grid supports under overhangs.
        #[arg(long, default_value_t = false)]
        support: bool,
        /// Overhang threshold from vertical, degrees.
        #[arg(long, default_value_t = 30.0)]
        support_angle: f64,
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
            cpu,
            gpu,
        } => {
            let pattern = InfillPattern::from_name(&infill_pattern)
                .ok_or(CliError::InfillPattern(infill_pattern))?;
            let seam = SeamPosition::from_name(&seam).ok_or(CliError::Seam(seam))?;
            let gcode = slice_file(
                &input,
                layer_height,
                infill,
                walls,
                pattern,
                seam,
                skirt,
                skirt_distance,
                brim,
                support,
                support_angle,
                cpu,
                gpu,
            )?;
            std::fs::write(&output, gcode)?;
            tracing::info!("wrote {}", output.display());
        }
    }
    Ok(())
}

#[allow(clippy::too_many_arguments)]
pub fn slice_file(
    input: &std::path::Path,
    layer_height: f64,
    infill: f64,
    walls: u32,
    infill_pattern: InfillPattern,
    seam: SeamPosition,
    skirt: u32,
    skirt_distance: f64,
    brim: f64,
    enable_support: bool,
    support_angle: f64,
    force_cpu: bool,
    force_gpu: bool,
) -> Result<String, CliError> {
    let mesh = load_stl(input)?;
    let settings = SliceSettings {
        layer_height_mm: layer_height,
        infill_density: infill,
        wall_loops: walls.max(1),
        infill_pattern,
        seam,
        skirt_loops: skirt,
        skirt_distance_mm: skirt_distance,
        brim_width_mm: brim.max(0.0),
        enable_support,
        support_threshold_angle_deg: support_angle.clamp(0.0, 89.0),
        ..Default::default()
    };

    let (sliced, backend) = if force_cpu {
        (slice_mesh(&mesh, &settings)?, SliceBackend::Cpu)
    } else if force_gpu {
        (slice_on_vulkan(&mesh, &settings)?, SliceBackend::VulkanCompute)
    } else {
        slice_with_gpu_or_cpu(&mesh, &settings)?
    };
    tracing::info!("sliced {} layers ({backend})", sliced.layers.len());
    Ok(write_gcode(&settings, &sliced)?)
}
