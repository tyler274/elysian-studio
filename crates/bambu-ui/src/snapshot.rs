//! JSON `GuiSnapshot` plus iced_wgpu headless PNG capture.

use std::io::Cursor;
use std::path::Path;

use iced::advanced::layout;
use iced::advanced::renderer::{self, Headless};
use iced::advanced::widget::Tree;
use iced::advanced::Layout;
use iced::mouse;
use iced::{Color, Font, Pixels, Rectangle, Size};
use serde::{Deserialize, Serialize};

use crate::{theme, App, ProcessTab, Workspace, SIDEBAR_WIDTH, WINDOW_SIZE};

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct GuiSnapshot {
    pub workspace: String,
    pub printer: String,
    pub process: String,
    pub process_search: String,
    pub process_objects: bool,
    pub filaments: Vec<FilamentSnap>,
    pub plate: usize,
    pub plate_name: String,
    pub plate_badge: String,
    pub quality_tab: String,
    pub fps_visible: bool,
    pub fps: u32,
    pub sidebar_width: u32,
    pub sidebar_collapsed: bool,
    pub printer_open: bool,
    pub filament_open: bool,
    pub process_open: bool,
    pub active_tab_fill: String,
    pub seam: String,
    pub sparse_infill_pct: u32,
    pub layer_height_mm: f64,
    pub first_layer_height_mm: f64,
    pub dual_nozzle_overlay: bool,
    pub labels: Vec<String>,
    pub window: [u32; 2],
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct FilamentSnap {
    pub name: String,
    pub colour: String,
}

impl App {
    pub fn snapshot(&self) -> GuiSnapshot {
        let filaments = self
            .filament_slots
            .iter()
            .map(|s| FilamentSnap {
                name: s.name.clone(),
                colour: s.colour.clone(),
            })
            .collect();
        GuiSnapshot {
            workspace: self.workspace.as_str().into(),
            printer: self.machine_name.clone().unwrap_or_else(|| "unset".into()),
            process: self.process_name.clone().unwrap_or_else(|| "—".into()),
            process_search: self.process_search.clone(),
            process_objects: self.process_objects,
            filaments,
            plate: self.plate,
            plate_name: self.plate_label(),
            plate_badge: format!("{:02}", self.plate + 1),
            quality_tab: self.quality_tab.as_str().into(),
            fps_visible: self.workspace == Workspace::Preview,
            fps: self.fps.round() as u32,
            sidebar_width: if self.sidebar_collapsed {
                0
            } else {
                SIDEBAR_WIDTH as u32
            },
            sidebar_collapsed: self.sidebar_collapsed,
            printer_open: self.printer_open,
            filament_open: self.filament_open,
            process_open: self.process_open,
            active_tab_fill: theme::active_tab_hex(self.workspace).into(),
            seam: self.settings.seam.as_str().into(),
            sparse_infill_pct: (self.settings.infill_density * 100.0).round() as u32,
            layer_height_mm: self.settings.layer_height_mm,
            first_layer_height_mm: self.settings.first_layer_height_mm,
            dual_nozzle_overlay: self.show_left_nozzle_only(),
            labels: vec![
                "Home".into(),
                "Prepare".into(),
                "Preview".into(),
                "Slice plate".into(),
                "Print plate".into(),
                "AMS".into(),
                "Printer".into(),
                "Filament".into(),
                "Process".into(),
                "Quality".into(),
                "Seam".into(),
                "Iso".into(),
                "Top".into(),
                "Front".into(),
            ],
            window: [WINDOW_SIZE.width as u32, WINDOW_SIZE.height as u32],
        }
    }

    /// Offscreen iced_wgpu screenshot at [`WINDOW_SIZE`]. `None` without an adapter.
    pub fn screenshot_rgba(&self) -> Option<Vec<u8>> {
        bambu_gpu::force_vulkan_env();
        let mut renderer = pollster::block_on(iced::Renderer::new(
            Font::DEFAULT,
            Pixels::from(theme::BODY_SIZE),
            Some("wgpu"),
        ))?;
        let mut ui = self.view();
        let bounds = Size::new(WINDOW_SIZE.width, WINDOW_SIZE.height);
        let mut tree = Tree::new(ui.as_widget());
        let node = ui.as_widget_mut().layout(
            &mut tree,
            &renderer,
            &layout::Limits::new(Size::ZERO, bounds),
        );
        let viewport = Rectangle::with_size(bounds);
        iced::advanced::Renderer::reset(&mut renderer, viewport);
        ui.as_widget().draw(
            &tree,
            &mut renderer,
            &theme::studio(),
            &renderer::Style {
                text_color: theme::TEXT,
            },
            Layout::new(&node),
            mouse::Cursor::Unavailable,
            &viewport,
        );
        Some(Headless::screenshot(
            &mut renderer,
            Size::new(WINDOW_SIZE.width as u32, WINDOW_SIZE.height as u32),
            1.0,
            theme::HEADER,
        ))
    }
}

impl Workspace {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Home => "home",
            Self::Prepare => "prepare",
            Self::Preview => "preview",
            Self::Device => "device",
            Self::Project => "project",
            Self::Calibration => "calibration",
            Self::Filament => "filament",
        }
    }
}

impl ProcessTab {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Quality => "quality",
            Self::Strength => "strength",
            Self::Speed => "speed",
            Self::Support => "support",
            Self::Others => "others",
        }
    }
}

pub fn encode_png(rgba: &[u8], width: u32, height: u32) -> Result<Vec<u8>, String> {
    let mut out = Vec::new();
    {
        let mut encoder = png::Encoder::new(&mut out, width, height);
        encoder.set_color(png::ColorType::Rgba);
        encoder.set_depth(png::BitDepth::Eight);
        let mut writer = encoder
            .write_header()
            .map_err(|e| format!("png header: {e}"))?;
        writer
            .write_image_data(rgba)
            .map_err(|e| format!("png data: {e}"))?;
    }
    Ok(out)
}

pub fn decode_png(bytes: &[u8]) -> Result<(u32, u32, Vec<u8>), String> {
    let decoder = png::Decoder::new(Cursor::new(bytes));
    let mut reader = decoder.read_info().map_err(|e| format!("png read: {e}"))?;
    let mut buf = vec![0; reader.output_buffer_size()];
    let info = reader
        .next_frame(&mut buf)
        .map_err(|e| format!("png frame: {e}"))?;
    buf.truncate(info.buffer_size());
    let rgba = match info.color_type {
        png::ColorType::Rgba => buf,
        png::ColorType::Rgb => buf
            .chunks_exact(3)
            .flat_map(|c| [c[0], c[1], c[2], 255])
            .collect(),
        other => return Err(format!("png color {other:?}")),
    };
    Ok((info.width, info.height, rgba))
}

pub fn write_png(path: &Path, rgba: &[u8], width: u32, height: u32) -> Result<(), String> {
    std::fs::write(path, encode_png(rgba, width, height)?).map_err(|e| e.to_string())
}

/// Nearest-neighbor scale of packed RGBA8. Used to compare upstream captures at 1200×800.
pub fn scale_rgba(src: &[u8], sw: u32, sh: u32, dw: u32, dh: u32) -> Vec<u8> {
    let out_len = (dw as usize).saturating_mul(dh as usize).saturating_mul(4);
    let mut out = vec![0u8; out_len];
    if sw == 0 || sh == 0 || dw == 0 || dh == 0 {
        return out;
    }
    for y in 0..dh {
        let sy = y * sh / dh;
        for x in 0..dw {
            let sx = x * sw / dw;
            let si = ((sy * sw + sx) * 4) as usize;
            let di = ((y * dw + x) * 4) as usize;
            if let Some(px) = src.get(si..si + 4) {
                out[di..di + 4].copy_from_slice(px);
            }
        }
    }
    out
}

/// GPU-tolerant compare: RMSE plus a max-channel delta.
pub fn png_delta(a: &[u8], b: &[u8]) -> (f64, u8) {
    let n = a.len().min(b.len()).max(1);
    let mut sse = 0.0;
    let mut max = 0u8;
    for (x, y) in a.iter().zip(b) {
        let d = i32::from(*x) - i32::from(*y);
        sse += f64::from(d * d);
        max = max.max(d.unsigned_abs() as u8);
    }
    ((sse / n as f64).sqrt(), max)
}

pub fn color_near(px: &[u8], target: Color, slop: u8) -> bool {
    if px.len() < 3 {
        return false;
    }
    let tr = (target.r * 255.0).round() as i16;
    let tg = (target.g * 255.0).round() as i16;
    let tb = (target.b * 255.0).round() as i16;
    (i16::from(px[0]) - tr).unsigned_abs() <= u16::from(slop)
        && (i16::from(px[1]) - tg).unsigned_abs() <= u16::from(slop)
        && (i16::from(px[2]) - tb).unsigned_abs() <= u16::from(slop)
}

fn chroma_matches_fill(px: &[u8], fill: Color) -> bool {
    if px.len() < 3 {
        return false;
    }
    let r = px[0];
    let g = px[1];
    let b = px[2];
    if fill.g > fill.b {
        g > 80 && g > r.saturating_add(25) && g > b
    } else {
        b > 60 && g > 60 && r < g.saturating_add(10) && r < 90
    }
}

/// First matching pixel in the header strip (top 56px).
pub fn header_has_fill(rgba: &[u8], width: u32, fill: Color) -> bool {
    let w = width as usize;
    for y in 0..56usize {
        for x in 0..w.saturating_sub(1) {
            let i = (y * w + x) * 4;
            if let Some(px) = rgba.get(i..i + 4) {
                if color_near(px, fill, 80) || chroma_matches_fill(px, fill) {
                    return true;
                }
            }
        }
    }
    false
}

pub fn sidebar_is_width(rgba: &[u8], width: u32, height: u32) -> bool {
    let w = width as usize;
    if w < 32 {
        return false;
    }
    let mut hits = 0u32;
    let mid = (height as usize / 2).min(height.saturating_sub(1) as usize);
    for y in mid.saturating_sub(40)..mid.saturating_add(40).min(height as usize) {
        for x in [
            w.saturating_sub(8),
            w.saturating_sub(16),
            w.saturating_sub(24),
        ] {
            if x >= w {
                continue;
            }
            let i = (y * w + x) * 4;
            if let Some(px) = rgba.get(i..i + 4) {
                if color_near(px, theme::SIDEBAR, 80)
                    || color_near(px, theme::CARD, 80)
                    || chroma_sidebar(px)
                {
                    hits += 1;
                }
            }
        }
    }
    hits >= 20
}

fn chroma_sidebar(px: &[u8]) -> bool {
    px.len() >= 3 && px[0] < 70 && px[1] < 70 && px[2] < 80 && px[1] >= px[0]
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn scale_rgba_identity() {
        let src = vec![1u8, 2, 3, 255, 4, 5, 6, 255];
        assert_eq!(scale_rgba(&src, 2, 1, 2, 1), src);
    }

    #[test]
    fn scale_rgba_nearest_up() {
        let src = vec![10u8, 20, 30, 255];
        let out = scale_rgba(&src, 1, 1, 2, 2);
        assert_eq!(out.len(), 16);
        assert_eq!(&out[0..4], &[10, 20, 30, 255]);
        assert_eq!(&out[12..16], &[10, 20, 30, 255]);
    }

    #[test]
    fn decode_png_expands_rgb() {
        let rgb = vec![1u8, 2, 3, 4, 5, 6];
        let mut out = Vec::new();
        {
            let mut encoder = png::Encoder::new(&mut out, 2, 1);
            encoder.set_color(png::ColorType::Rgb);
            encoder.set_depth(png::BitDepth::Eight);
            let mut writer = encoder.write_header().unwrap();
            writer.write_image_data(&rgb).unwrap();
        }
        let (w, h, rgba) = decode_png(&out).expect("rgb png");
        assert_eq!((w, h), (2, 1));
        assert_eq!(rgba, vec![1, 2, 3, 255, 4, 5, 6, 255]);
    }
}
