//! Z-up orbit camera for the plater viewport.

use glam::{Mat4, Vec3};

/// Named plater camera presets (Studio view cube faces, not ImGuizmo).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CameraView {
    Iso,
    Top,
    Front,
    Back,
    Left,
    Right,
    Fit,
}

#[derive(Debug, Clone, Copy)]
pub struct OrbitCamera {
    pub yaw: f32,
    pub pitch: f32,
    pub distance: f32,
    pub target: Vec3,
}

impl OrbitCamera {
    pub const PITCH_MIN: f32 = 0.08;
    pub const PITCH_MAX: f32 = 1.45;
    pub const ISO_YAW: f32 = 0.65;
    pub const ISO_PITCH: f32 = 0.55;
    pub const FOV: f32 = std::f32::consts::FRAC_PI_4;

    pub fn looking_at_bed(bed_mm: f32) -> Self {
        Self::looking_at_center(Vec3::new(bed_mm * 0.5, bed_mm * 0.5, 0.0), bed_mm)
    }

    pub fn looking_at_center(target: Vec3, span_mm: f32) -> Self {
        Self {
            yaw: Self::ISO_YAW,
            pitch: Self::ISO_PITCH,
            distance: span_mm * 1.65,
            target,
        }
    }

    /// Studio view-cube faces. Pitch stays in [`Self::PITCH_MIN`]..=[`Self::PITCH_MAX`].
    pub fn apply_view(&mut self, view: CameraView, target: Vec3, span_mm: f32) {
        match view {
            CameraView::Iso => {
                self.yaw = Self::ISO_YAW;
                self.pitch = Self::ISO_PITCH;
            }
            CameraView::Top => {
                self.pitch = Self::PITCH_MAX;
            }
            CameraView::Front => {
                self.yaw = -std::f32::consts::FRAC_PI_2;
                self.pitch = Self::PITCH_MIN;
            }
            CameraView::Back => {
                self.yaw = std::f32::consts::FRAC_PI_2;
                self.pitch = Self::PITCH_MIN;
            }
            CameraView::Left => {
                self.yaw = std::f32::consts::PI;
                self.pitch = Self::PITCH_MIN;
            }
            CameraView::Right => {
                self.yaw = 0.0;
                self.pitch = Self::PITCH_MIN;
            }
            CameraView::Fit => *self = Self::looking_at_center(target, span_mm),
        }
    }

    /// Near/far that keep a framed model inside the clip volume.
    pub fn clip_range(self) -> (f32, f32) {
        let near = (self.distance * 0.005).clamp(0.1, 5.0);
        let far = (self.distance * 12.0).max(4000.0);
        (near, far)
    }

    pub fn perspective(self, aspect: f32) -> Mat4 {
        let (near, far) = self.clip_range();
        Mat4::perspective_rh(Self::FOV, aspect.max(0.1), near, far)
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
        self.pitch = (self.pitch + dy * 0.008).clamp(Self::PITCH_MIN, Self::PITCH_MAX);
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
        let proj = self.perspective(aspect);
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

    #[test]
    fn top_looks_down_plus_z() {
        let mut cam = OrbitCamera::looking_at_center(Vec3::new(128.0, 128.0, 0.0), 256.0);
        cam.apply_view(CameraView::Top, cam.target, 256.0);
        assert!(cam.pitch >= OrbitCamera::PITCH_MAX - 1e-5);
        assert!(
            cam.eye().z > cam.target.z + 10.0,
            "top view eye should sit above the bed, eye.z={} target.z={}",
            cam.eye().z,
            cam.target.z
        );
    }

    #[test]
    fn front_is_not_the_iso_default() {
        let iso = OrbitCamera::looking_at_center(Vec3::ZERO, 256.0);
        let mut front = iso;
        front.apply_view(CameraView::Front, iso.target, 256.0);
        assert!((front.yaw - iso.yaw).abs() > 0.2);
        assert!((front.pitch - iso.pitch).abs() > 0.2);
    }

    #[test]
    fn clip_range_grows_with_distance() {
        let near = OrbitCamera::looking_at_center(Vec3::ZERO, 100.0);
        let far = OrbitCamera::looking_at_center(Vec3::ZERO, 2000.0);
        assert!(far.clip_range().1 > near.clip_range().1);
        assert!(far.clip_range().1 > far.distance);
    }
}
