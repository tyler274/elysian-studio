//! Printable plate polygons and dual-nozzle “only” strips (Studio `PartPlate`).

use crate::SliceSettings;

/// Axis-aligned rectangle on the bed (mm).
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct BedRect {
    pub x: f32,
    pub y: f32,
    pub w: f32,
    pub h: f32,
}

impl BedRect {
    pub fn max_x(self) -> f32 {
        self.x + self.w
    }

    pub fn max_y(self) -> f32 {
        self.y + self.h
    }
}

/// Printable plate used by the viewport (not the square `bed_size_mm` AABB).
#[derive(Debug, Clone, PartialEq)]
pub struct BedShape {
    pub printable: Vec<(f32, f32)>,
    pub exclude: Vec<(f32, f32)>,
    pub extruder_areas: Vec<Vec<(f32, f32)>>,
}

impl Default for BedShape {
    fn default() -> Self {
        Self::square(256.0)
    }
}

impl BedShape {
    pub fn square(mm: f32) -> Self {
        let mm = mm.max(1.0);
        Self {
            printable: vec![(0.0, 0.0), (mm, 0.0), (mm, mm), (0.0, mm)],
            exclude: Vec::new(),
            extruder_areas: Vec::new(),
        }
    }

    pub fn from_settings(settings: &SliceSettings) -> Self {
        let printable = poly_f32(&settings.printable_area);
        if printable.len() < 3 {
            return Self::square(settings.bed_size_mm());
        }
        Self {
            printable,
            exclude: poly_f32(&settings.bed_exclude_area),
            extruder_areas: settings
                .extruder_printable_areas
                .iter()
                .map(|p| poly_f32(p))
                .filter(|p| p.len() >= 3)
                .collect(),
        }
    }

    pub fn printable_aabb(&self) -> (f32, f32, f32, f32) {
        poly_aabb(&self.printable).unwrap_or((0.0, 0.0, 256.0, 256.0))
    }

    pub fn width(&self) -> f32 {
        let (x0, _, x1, _) = self.printable_aabb();
        (x1 - x0).max(1.0)
    }

    pub fn height(&self) -> f32 {
        let (_, y0, _, y1) = self.printable_aabb();
        (y1 - y0).max(1.0)
    }

    pub fn center(&self) -> (f32, f32) {
        let (x0, y0, x1, y1) = self.printable_aabb();
        ((x0 + x1) * 0.5, (y0 + y1) * 0.5)
    }

    /// Span used for the orbit camera (max edge).
    pub fn orbit_mm(&self) -> f32 {
        self.width().max(self.height()).clamp(80.0, 512.0)
    }

    /// Studio `PartPlateList::calc_extruder_only_area` for two logical nozzles.
    pub fn extruder_only_rects(&self) -> Option<(BedRect, BedRect)> {
        if self.extruder_areas.len() != 2 {
            return None;
        }
        let printable = aabb_rect(&self.printable)?;
        let left = aabb_rect(&self.extruder_areas[0])?;
        let right = aabb_rect(&self.extruder_areas[1])?;
        let left_only = BedRect {
            x: left.x,
            y: left.y,
            w: printable.w - right.w,
            h: left.h,
        };
        let right_only = BedRect {
            x: left.max_x(),
            y: right.y,
            w: printable.w - left.w,
            h: right.h,
        };
        if left_only.w < 0.0 || right_only.w < 0.0 {
            return None;
        }
        Some((left_only, right_only))
    }

    /// Studio skips the right SVG when the band is 5 mm or thinner.
    pub fn visible_only_rects(&self) -> (Option<BedRect>, Option<BedRect>) {
        match self.extruder_only_rects() {
            Some((left, right)) => (
                (left.w >= 5.0).then_some(left),
                (right.w > 5.0).then_some(right),
            ),
            None => (None, None),
        }
    }
}

impl SliceSettings {
    pub fn bed_shape(&self) -> BedShape {
        BedShape::from_settings(self)
    }
}

fn poly_f32(pts: &[(f64, f64)]) -> Vec<(f32, f32)> {
    pts.iter().map(|&(x, y)| (x as f32, y as f32)).collect()
}

fn poly_aabb(pts: &[(f32, f32)]) -> Option<(f32, f32, f32, f32)> {
    let mut iter = pts.iter().copied();
    let (x0, y0) = iter.next()?;
    let mut min_x = x0;
    let mut max_x = x0;
    let mut min_y = y0;
    let mut max_y = y0;
    for (x, y) in iter {
        min_x = min_x.min(x);
        max_x = max_x.max(x);
        min_y = min_y.min(y);
        max_y = max_y.max(y);
    }
    Some((min_x, min_y, max_x, max_y))
}

fn aabb_rect(pts: &[(f32, f32)]) -> Option<BedRect> {
    let (x0, y0, x1, y1) = poly_aabb(pts)?;
    Some(BedRect {
        x: x0,
        y: y0,
        w: x1 - x0,
        h: y1 - y0,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn h2c_bed() -> BedShape {
        BedShape {
            printable: vec![(0.0, 0.0), (330.0, 0.0), (330.0, 320.0), (0.0, 320.0)],
            exclude: Vec::new(),
            extruder_areas: vec![
                vec![(0.0, 0.0), (325.0, 0.0), (325.0, 320.0), (0.0, 320.0)],
                vec![(25.0, 0.0), (330.0, 0.0), (330.0, 320.0), (25.0, 320.0)],
            ],
        }
    }

    #[test]
    fn h2c_left_only_is_25mm_right_skipped() {
        let bed = h2c_bed();
        let (left, right) = bed.extruder_only_rects().expect("dual nozzle");
        assert!((left.w - 25.0).abs() < 1e-3, "left-only {}", left.w);
        assert!((left.x - 0.0).abs() < 1e-3);
        assert!((left.h - 320.0).abs() < 1e-3);
        assert!((right.w - 5.0).abs() < 1e-3, "right-only {}", right.w);
        assert!((right.x - 325.0).abs() < 1e-3);
        let (vis_l, vis_r) = bed.visible_only_rects();
        assert!(vis_l.is_some());
        assert!(vis_r.is_none(), "Studio hides the right SVG when width ≤ 5");
    }

    #[test]
    fn single_nozzle_has_no_only_rects() {
        let bed = BedShape::square(256.0);
        assert!(bed.extruder_only_rects().is_none());
        assert_eq!(bed.visible_only_rects(), (None, None));
    }
}
