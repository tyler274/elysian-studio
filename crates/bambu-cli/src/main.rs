#![forbid(unsafe_code)]

use std::path::PathBuf;

use bambu_config::SliceSettings;
use bambu_gcode::write_gcode;
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
    StdIo(#[from] std::io::Error),
}

#[derive(Parser)]
#[command(name = "bambu-cli", about = "Headless Bambu Studio slicer (Rust rewrite)")]
struct Cli {
    #[command(subcommand)]
    command: Commands,
}

#[derive(Subcommand)]
enum Commands {
    /// Slice an STL to G-code (horizontal contours + rectilinear infill).
    Slice {
        input: PathBuf,
        #[arg(short, long)]
        output: PathBuf,
        #[arg(long, default_value_t = 0.2)]
        layer_height: f64,
        #[arg(long, default_value_t = 0.20)]
        infill: f64,
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
        } => {
            let gcode = slice_file(&input, layer_height, infill)?;
            std::fs::write(&output, gcode)?;
            tracing::info!("wrote {}", output.display());
        }
    }
    Ok(())
}

pub fn slice_file(
    input: &std::path::Path,
    layer_height: f64,
    infill: f64,
) -> Result<String, CliError> {
    let mesh = load_stl(input)?;
    let mut settings = SliceSettings::default();
    settings.layer_height_mm = layer_height;
    settings.infill_density = infill;
    let sliced = slice_mesh(&mesh, &settings)?;
    tracing::info!("sliced {} layers", sliced.layers.len());
    Ok(write_gcode(&settings, &sliced)?)
}
