//! Bed-plate captions rasterized from the bundled Noto Sans subset.

use std::collections::HashMap;
use std::sync::OnceLock;

use ab_glyph::{Font, FontRef, PxScale, ScaleFont};
use bambu_config::BedRect;
use bytemuck::{Pod, Zeroable};

const TTF: &[u8] = include_bytes!("../fonts/NotoSans-Regular.ttf");
const PX: f32 = 48.0;
const PAD: u32 = 3;
const COLS: u32 = 8;
const COPY_ALIGN: u32 = 256;
const CHARSET: &str = " ABCDEFGHIJKLMNOPQRSTUVWXYZ";

#[repr(C)]
#[derive(Clone, Copy, Debug, Pod, Zeroable)]
pub struct LabelVertex {
    pub position: [f32; 3],
    pub uv: [f32; 2],
    pub color: [f32; 3],
}

#[derive(Clone, Copy)]
struct GlyphSlot {
    u0: f32,
    v0: f32,
    u1: f32,
    v1: f32,
    min_x: f32,
    min_y: f32,
    max_x: f32,
    max_y: f32,
}

pub struct GlyphAtlas {
    pub width: u32,
    pub height: u32,
    pub stride: u32,
    pub pixels: Vec<u8>,
    glyphs: HashMap<char, GlyphSlot>,
}

pub fn atlas() -> &'static GlyphAtlas {
    static ATLAS: OnceLock<GlyphAtlas> = OnceLock::new();
    ATLAS.get_or_init(GlyphAtlas::bake)
}

impl GlyphAtlas {
    fn bake() -> Self {
        let font = FontRef::try_from_slice(TTF).expect("bundled NotoSans-Regular.ttf");
        let scale = PxScale::from(PX);
        let scaled = font.as_scaled(scale);

        let mut max_w = 1.0_f32;
        let mut max_h = 1.0_f32;
        for ch in CHARSET.chars() {
            let Some(outlined) = font.outline_glyph(scaled.scaled_glyph(ch)) else {
                continue;
            };
            let b = outlined.px_bounds();
            max_w = max_w.max(b.width());
            max_h = max_h.max(b.height());
        }
        let cell_w = (max_w.ceil() as u32 + PAD * 2).max(1);
        let cell_h = (max_h.ceil() as u32 + PAD * 2).max(1);
        let n = CHARSET.chars().count() as u32;
        let rows = n.div_ceil(COLS);
        let width = (COLS * cell_w).max(1);
        let height = (rows * cell_h).max(1);
        let stride = width.next_multiple_of(COPY_ALIGN).max(COPY_ALIGN);
        let mut pixels = vec![0u8; (stride * height) as usize];
        let mut glyphs = HashMap::new();

        for (i, ch) in CHARSET.chars().enumerate() {
            let Some(outlined) = font.outline_glyph(scaled.scaled_glyph(ch)) else {
                continue;
            };
            let b = outlined.px_bounds();
            let col = i as u32 % COLS;
            let row = i as u32 / COLS;
            let ox = col * cell_w + PAD;
            let oy = row * cell_h + PAD;
            outlined.draw(|x, y, c| {
                let px = ox + x;
                let py = oy + y;
                if px < width && py < height {
                    let idx = (py * stride + px) as usize;
                    pixels[idx] = (c.clamp(0.0, 1.0) * 255.0).round() as u8;
                }
            });
            glyphs.insert(
                ch,
                GlyphSlot {
                    u0: ox as f32 / stride as f32,
                    v0: oy as f32 / height as f32,
                    u1: (ox as f32 + b.width()) / stride as f32,
                    v1: (oy as f32 + b.height()) / height as f32,
                    min_x: b.min.x,
                    min_y: b.min.y,
                    max_x: b.max.x,
                    max_y: b.max.y,
                },
            );
        }

        Self {
            width: stride,
            height,
            stride,
            pixels,
            glyphs,
        }
    }

    /// Stack `text` down the strip: first glyph at high Y (back of the plate).
    pub fn quads(&self, rect: BedRect, text: &str, color: [f32; 3]) -> Vec<LabelVertex> {
        let chars: Vec<char> = text.chars().collect();
        if chars.is_empty() || rect.w < 2.0 || rect.h < 8.0 {
            return Vec::new();
        }
        let em_mm = (rect.w * 0.72).clamp(3.5, 12.0);
        let mm_per_px = em_mm / PX;
        let line = em_mm * 1.18;
        let total = line * chars.len() as f32;
        let top = rect.y + (rect.h - total).max(0.0) * 0.5 + total;
        let mut baseline = top - em_mm * 0.82;
        let mut out = Vec::new();
        for ch in chars {
            if let Some(slot) = self.glyphs.get(&ch) {
                let gw = (slot.max_x - slot.min_x) * mm_per_px;
                let pen_x = rect.x + (rect.w - gw) * 0.5 - slot.min_x * mm_per_px;
                push_glyph(&mut out, slot, pen_x, baseline, mm_per_px, color);
            }
            baseline -= line;
        }
        out
    }
}

fn push_glyph(
    out: &mut Vec<LabelVertex>,
    slot: &GlyphSlot,
    pen_x: f32,
    baseline: f32,
    mm_per_px: f32,
    color: [f32; 3],
) {
    let x0 = pen_x + slot.min_x * mm_per_px;
    let x1 = pen_x + slot.max_x * mm_per_px;
    // ab_glyph px y grows downward from the baseline.
    let y_top = baseline - slot.min_y * mm_per_px;
    let y_bot = baseline - slot.max_y * mm_per_px;
    let z = 0.14_f32;
    let u0 = slot.u0;
    let v0 = slot.v0;
    let u1 = slot.u1;
    let v1 = slot.v1;
    let verts = [
        LabelVertex {
            position: [x0, y_bot, z],
            uv: [u0, v1],
            color,
        },
        LabelVertex {
            position: [x1, y_bot, z],
            uv: [u1, v1],
            color,
        },
        LabelVertex {
            position: [x1, y_top, z],
            uv: [u1, v0],
            color,
        },
        LabelVertex {
            position: [x0, y_bot, z],
            uv: [u0, v1],
            color,
        },
        LabelVertex {
            position: [x1, y_top, z],
            uv: [u1, v0],
            color,
        },
        LabelVertex {
            position: [x0, y_top, z],
            uv: [u0, v0],
            color,
        },
    ];
    out.extend_from_slice(&verts);
}

pub fn plate_labels(bed: &bambu_config::BedShape, color: [f32; 3]) -> Vec<LabelVertex> {
    let mut out = Vec::new();
    let (left, right) = bed.visible_only_rects();
    if let Some(rect) = left {
        out.extend(atlas().quads(rect, "LEFT NOZZLE ONLY", color));
    }
    if let Some(rect) = right {
        out.extend(atlas().quads(rect, "RIGHT NOZZLE ONLY", color));
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;
    use bambu_config::BedShape;

    #[test]
    fn atlas_contains_plate_letters() {
        let atlas = atlas();
        assert!(atlas.width >= 256);
        assert!(!atlas.pixels.iter().all(|p| *p == 0), "atlas is empty");
        for ch in "LEFTNOZZLEONLY".chars() {
            assert!(atlas.glyphs.contains_key(&ch), "missing glyph {ch}");
        }
    }

    #[test]
    fn stacked_label_reads_back_to_front() {
        let rect = BedRect {
            x: 0.0,
            y: 0.0,
            w: 25.0,
            h: 320.0,
        };
        let verts = atlas().quads(rect, "LY", [1.0, 1.0, 1.0]);
        assert_eq!(verts.len(), 12);
        let l_y: f32 = verts[..6].iter().map(|v| v.position[1]).sum::<f32>() / 6.0;
        let y_y: f32 = verts[6..].iter().map(|v| v.position[1]).sum::<f32>() / 6.0;
        assert!(
            l_y > y_y + 5.0,
            "L (first) should sit at higher Y than Y (last): {l_y} vs {y_y}"
        );
    }

    #[test]
    fn h2c_bed_emits_left_only_label() {
        let bed = BedShape {
            printable: vec![(0.0, 0.0), (330.0, 0.0), (330.0, 320.0), (0.0, 320.0)],
            exclude: Vec::new(),
            extruder_areas: vec![
                vec![(0.0, 0.0), (325.0, 0.0), (325.0, 320.0), (0.0, 320.0)],
                vec![(25.0, 0.0), (330.0, 0.0), (330.0, 320.0), (25.0, 320.0)],
            ],
        };
        let verts = plate_labels(&bed, [0.75, 0.75, 0.75]);
        assert!(
            verts.len() >= 6 * "LEFT NOZZLE ONLY".chars().filter(|c| *c != ' ').count(),
            "expected textured quads for LEFT NOZZLE ONLY, got {}",
            verts.len()
        );
    }
}
