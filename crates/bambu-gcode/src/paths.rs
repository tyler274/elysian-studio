//! Extrusion path emission: roles, overhang classification, small-perimeter slowdown.

use bambu_config::SliceSettings;
use bambu_geom::{offset_polygons, Polygon, Polyline};
use bambu_slicer::{classify_overhang, ClassifiedPath, Layer};

use crate::motion::Writer;
use crate::GcodeError;

/// Shared extrusion parameters for a batch of paths on the current layer.
pub(crate) struct Extrude<'a> {
    pub paths: &'a [Polyline],
    pub closed: bool,
    pub e_per_mm: f64,
    pub print_f: f64,
    pub mm3_per_mm: f64,
}

impl Writer<'_> {
    pub(crate) fn emit_paths(&mut self, job: Extrude<'_>) -> Result<(), GcodeError> {
        let print_f = self
            .settings
            .cap_extrude_feed_mm_min(job.print_f, job.mm3_per_mm);
        for path in job.paths {
            self.emit_one_path(path, job.closed, job.e_per_mm, print_f, false)?;
        }
        Ok(())
    }

    pub(crate) fn emit_wall_paths(
        &mut self,
        supported_feature: &str,
        job: Extrude<'_>,
        support: Option<&[Vec<Polygon>]>,
        slow_overhang: bool,
        apply_small: bool,
    ) -> Result<(), GcodeError> {
        if job.paths.is_empty() {
            return Ok(());
        }
        let mut current_feature: Option<&str> = None;
        for path in job.paths {
            if path.len() < 2 {
                continue;
            }
            let runs = match support {
                Some(rings) if !rings.is_empty() => classify_overhang(path, rings, job.closed),
                _ => vec![ClassifiedPath {
                    path: path.clone(),
                    degree: 0,
                }],
            };
            let single = runs.len() == 1;
            for run in &runs {
                if run.path.len() < 2 {
                    continue;
                }
                let total = !run.inside() && slow_overhang;
                let feature = if total {
                    "Overhang wall"
                } else {
                    supported_feature
                };
                if current_feature != Some(feature) {
                    self.emit_feature(feature)?;
                    current_feature = Some(feature);
                }
                let mut feed = overhang_feed(self.settings, run.degree, job.print_f);
                if apply_small {
                    feed = small_perimeter_feed(self.settings, path, job.closed, feed);
                }
                feed = self.settings.cap_extrude_feed_mm_min(feed, job.mm3_per_mm);
                let boost = self.settings.overhang_fan_applies(
                    run.degree,
                    false,
                    supported_feature == "Outer wall",
                );
                self.emit_marked(boost, ";_OVERHANG_FAN_START", ";_OVERHANG_FAN_END", |w| {
                    w.emit_one_path(
                        &run.path,
                        job.closed && single,
                        job.e_per_mm,
                        feed,
                        supported_feature == "Outer wall",
                    )
                })?;
            }
        }
        Ok(())
    }
}

pub(crate) fn overhang_rings(
    settings: &SliceSettings,
    lower: Option<&Layer>,
) -> Option<Vec<Vec<Polygon>>> {
    if !settings.detect_overhang_wall {
        return None;
    }
    let lower = lower?;
    if lower.contours.is_empty() {
        return None;
    }
    let start = -0.5 * settings.line_width_mm;
    let end = 0.5 * settings.nozzle_diameter_mm;
    let mut rings = Vec::with_capacity(5);
    for i in 0..5 {
        let t = f64::from(i) / 4.0;
        let offset = start + t * (end - start);
        let grown = offset_polygons(&lower.contours, offset);
        rings.push(if grown.is_empty() {
            lower.contours.clone()
        } else {
            grown
        });
    }
    Some(rings)
}

fn overhang_feed(settings: &SliceSettings, degree: u8, print_f: f64) -> f64 {
    if !settings.enable_overhang_speed || degree == 0 {
        return print_f;
    }
    let band = match degree {
        1 => settings.overhang_1_4_speed_mm_s,
        2 => settings.overhang_2_4_speed_mm_s,
        3 => settings.overhang_3_4_speed_mm_s,
        4 => settings.overhang_4_4_speed_mm_s,
        _ => settings.overhang_speed_mm_s,
    };
    if band <= 0.0 {
        print_f
    } else {
        band * 60.0
    }
}

/// C++ `SMALL_PERIMETER_LENGTH`: circumference of a circle with the given radius.
fn small_perimeter_max_length_mm(threshold_mm: f64) -> f64 {
    std::f64::consts::TAU * threshold_mm
}

fn polyline_length_mm(path: &[bambu_geom::Point], closed: bool) -> f64 {
    if path.len() < 2 {
        return 0.0;
    }
    let mut len = 0.0;
    for window in path.windows(2) {
        len += window[0].distance_mm(window[1]);
    }
    if closed {
        len += path[path.len() - 1].distance_mm(path[0]);
    }
    len
}

/// C++ `extrude_loop`: if the whole loop is a small perimeter, take `min` with the role speed.
fn small_perimeter_feed(
    settings: &SliceSettings,
    path: &[bambu_geom::Point],
    closed: bool,
    print_f: f64,
) -> f64 {
    let speed = settings.small_perimeter_speed_mm_s();
    if speed <= 0.0 {
        return print_f;
    }
    if polyline_length_mm(path, closed)
        > small_perimeter_max_length_mm(settings.small_perimeter_threshold_mm)
    {
        return print_f;
    }
    print_f.min(speed * 60.0)
}
