//! G-code writer and processor.

#![forbid(unsafe_code)]

mod cooling;
mod envelope;
mod motion;
mod parse;
mod paths;
mod processor;
mod timelapse;
mod writer;

#[cfg(test)]
mod tests;

use thiserror::Error;

pub use cooling::{
    additional_fan_percent, apply_layer_cooling_slowdown, apply_part_cooling, part_fan_percent,
    set_additional_fan_gcode, set_exhaust_fan_gcode, set_fan_gcode,
};
pub use parse::{
    assert_matches_cpp, assert_matches_cpp_with, layer_stats, parse_config_comments, parse_gcode,
    GcodeReport, LayerStats,
};
pub use processor::{format_time_dhms, process_gcode, ProcessorResult};
pub use writer::write_gcode;

#[derive(Debug, Error)]
pub enum GcodeError {
    #[error("format: {0}")]
    Format(#[from] std::fmt::Error),
}
