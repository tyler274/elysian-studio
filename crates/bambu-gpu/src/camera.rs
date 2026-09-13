//! Z-up orbit camera for the plater viewport.

use glam::{Mat4, Vec3};

#[derive(Debug, Clone, Copy)]
pub struct OrbitCamera {
    pub yaw: f32,
    pub pitch: f32,
    pub distance: f32,
    pub target: Vec3,
}

impl OrbitCamera {
    pub fn looking_at_bed(bed_mm: f32) -> Self {
        Self {
            yaw: 0.65,
            pitch: 0.55,
            distance: bed_mm * 1.65,
            target: Vec3::new(bed_mm * 0.5, bed_mm * 0.5, 0.0),
        }
    }

    pub fn eye(self) -> Vec3 {
        let cp = self.pitch.cos();
        let dir = Vec3::new(self.yaw.cos() * cp, self.yaw.sin() * cp, self.pitch.sin());
        self.target + dir * self.distance
    }

    pub fn view_matrix(self) -> Mat4 {
        Mat4::look_at_rh(self.eye(), self.target, Vec3::Z)
    }

    pub fn orbit(&mut self, dx: f32, dy: f32) {
        self.yaw += dx * 0.008;
        self.pitch = (self.pitch + dy * 0.008).clamp(0.08, 1.45);
    }

    pub fn zoom(&mut self, scroll_lines: f32) {
        let factor = (1.0 - scroll_lines * 0.08).clamp(0.5, 1.5);
        self.distance = (self.distance * factor).clamp(40.0, 2500.0);
    }

    pub fn ray_from_ndc(self, ndc_x: f32, ndc_y: f32, aspect: f32) -> (Vec3, Vec3) {
        let proj = Mat4::perspective_rh(std::f32::consts::FRAC_PI_4, aspect.max(0.1), 1.0, 4000.0);
        let view = self.view_matrix();
        let inv = (proj * view).inverse();
        let n = inv * glam::Vec4::new(ndc_x, ndc_y, 0.0, 1.0);
        let f = inv * glam::Vec4::new(ndc_x, ndc_y, 1.0, 1.0);
        let a = n.truncate() / n.w.max(1e-8);
        let b = f.truncate() / f.w.max(1e-8);
        let dir = (b - a).normalize_or_zero();
        (a, dir)
    }
}
