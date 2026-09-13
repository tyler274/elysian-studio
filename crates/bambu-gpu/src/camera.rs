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
        Self::looking_at_center(Vec3::new(bed_mm * 0.5, bed_mm * 0.5, 0.0), bed_mm)
    }

    pub fn looking_at_center(target: Vec3, span_mm: f32) -> Self {
        Self {
            yaw: 0.65,
            pitch: 0.55,
            distance: span_mm * 1.65,
            target,
        }
    }

    /// Ray ∩ z=0 plane, or `None` if parallel / behind the camera.
    pub fn hit_z0(self, ndc_x: f32, ndc_y: f32, aspect: f32) -> Option<Vec3> {
        let (origin, dir) = self.ray_from_ndc(ndc_x, ndc_y, aspect);
        if dir.z.abs() < 1e-8 {
            return None;
        }
        let t = -origin.z / dir.z;
        (t > 1e-4).then_some(origin + dir * t)
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

    /// Studio middle-drag: slide the look-at point on the bed (z=0).
    pub fn pan_xy(&mut self, dx: f32, dy: f32) {
        self.target.x += dx;
        self.target.y += dy;
    }

    /// Fallback when the pointer ray misses the bed: pan in camera right/up.
    /// `dx`/`dy` are iced pixels (y increases downward).
    pub fn pan_screen(&mut self, dx: f32, dy: f32) {
        let forward = (self.target - self.eye()).normalize_or_zero();
        let right = {
            let r = forward.cross(Vec3::Z);
            if r.length_squared() < 1e-8 {
                Vec3::X
            } else {
                r.normalize()
            }
        };
        let cam_up = right.cross(forward).normalize_or_zero();
        let k = self.distance * 0.0025;
        self.target -= right * dx * k;
        self.target -= cam_up * dy * k;
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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn pan_xy_slides_the_look_at_on_the_bed() {
        let mut cam = OrbitCamera::looking_at_bed(256.0);
        let t0 = cam.target;
        cam.pan_xy(10.0, -4.0);
        assert!((cam.target.x - t0.x - 10.0).abs() < 1e-5);
        assert!((cam.target.y - t0.y + 4.0).abs() < 1e-5);
        assert!((cam.target.z - t0.z).abs() < 1e-5);
    }

    #[test]
    fn pan_screen_right_moves_target_left_of_camera() {
        let mut cam = OrbitCamera::looking_at_center(Vec3::new(128.0, 128.0, 0.0), 256.0);
        cam.yaw = -std::f32::consts::FRAC_PI_2;
        cam.pitch = 0.9;
        let t0 = cam.target;
        cam.pan_screen(40.0, 0.0);
        assert!(
            cam.target.x < t0.x,
            "dragging right should pan the bed left, got {} vs {}",
            cam.target.x,
            t0.x
        );
    }
}
