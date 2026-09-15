//! Bed-plate captions rasterized from the bundled Noto Sans subset.

use std::collections::HashMap;
use std::sync::OnceLock;

use ab_glyph::{Font, FontRef, PxScale, ScaleFont};
use elysian_config::BedRect;
use bytemuck::{Pod, Zeroable};

const TTF: &[u8] = include_bytes!("../fonts/NotoSans-Regular.ttf");
const PX: f32 = 48.0;
const PAD: u32 = 3;
const COLS: u32 = 8;
const COPY_ALIGN: u32 = 256;
const CHARSET: &str = " ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz";
pub const LEFT_ONLY_CAPTION: &str = "Left nozzle only area";
pub const RIGHT_ONLY_CAPTION: &str = "Right nozzle only area";

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
    advance: f32,
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
            let glyph = scaled.scaled_glyph(ch);
            let advance = scaled.h_advance(glyph.id);
            let col = i as u32 % COLS;
            let row = i as u32 / COLS;
            let ox = col * cell_w + PAD;
            let oy = row * cell_h + PAD;
            let mut slot = GlyphSlot {
                u0: 0.0,
                v0: 0.0,
                u1: 0.0,
                v1: 0.0,
                min_x: 0.0,
                min_y: 0.0,
                max_x: 0.0,
                max_y: 0.0,
                advance,
            };
            if let Some(outlined) = font.outline_glyph(glyph) {
                let b = outlined.px_bounds();
                outlined.draw(|x, y, c| {
                    let px = ox + x;
                    let py = oy + y;
                    if px < width && py < height {
                        let idx = (py * stride + px) as usize;
                        pixels[idx] = (c.clamp(0.0, 1.0) * 255.0).round() as u8;
                    }
                });
                slot.u0 = ox as f32 / stride as f32;
                slot.v0 = oy as f32 / height as f32;
                slot.u1 = (ox as f32 + b.width()) / stride as f32;
                slot.v1 = (oy as f32 + b.height()) / height as f32;
                slot.min_x = b.min.x;
                slot.min_y = b.min.y;
                slot.max_x = b.max.x;
                slot.max_y = b.max.y;
            }
            glyphs.insert(ch, slot);
        }

        Self {
            width: stride,
            height,
            stride,
            pixels,
            glyphs,
        }
    }

    fn glyph(&self, ch: char) -> Option<&GlyphSlot> {
        self.glyphs
            .get(&ch)
            .or_else(|| self.glyphs.get(&ch.to_ascii_uppercase()))
    }

    /// Studio `left_extruder_only_area.svg`: one mixed-case line rotated 90° CCW
    /// so it reads from the front of the plate (+Y).
    pub fn quads(&self, rect: BedRect, text: &str, color: [f32; 3]) -> Vec<LabelVertex> {
        if text.chars().all(|c| c.is_whitespace()) || rect.w < 2.0 || rect.h < 8.0 {
            return Vec::new();
        }
        let em_mm = (rect.w * 0.48).clamp(3.5, 11.0);
        let mm_per_px = em_mm / PX;
        let mut width_mm = 0.0_f32;
        for ch in text.chars() {
            if let Some(slot) = self.glyph(ch) {
                width_mm += slot.advance * mm_per_px;
            }
        }
        if width_mm < 1.0 {
            return Vec::new();
        }
        let cx = rect.x + rect.w * 0.5;
        let cy = rect.y + rect.h * 0.5;
        let mut pen = -width_mm * 0.5;
        let mut out = Vec::new();
        for ch in text.chars() {
            let Some(slot) = self.glyph(ch) else {
                continue;
            };
            if !ch.is_whitespace() {
                push_glyph_rotated(&mut out, slot, pen, mm_per_px, cx, cy, color);
            }
            pen += slot.advance * mm_per_px;
        }
        out
    }
}

fn rot90_ccw(local_x: f32, local_y: f32, cx: f32, cy: f32) -> [f32; 2] {
    [cx - local_y, cy + local_x]
}

fn push_glyph_rotated(
    out: &mut Vec<LabelVertex>,
    slot: &GlyphSlot,
    pen: f32,
    mm_per_px: f32,
    cx: f32,
    cy: f32,
    color: [f32; 3],
) {
    let x0 = pen + slot.min_x * mm_per_px;
    let x1 = pen + slot.max_x * mm_per_px;
    // ab_glyph px y grows downward from the baseline.
    let y_top = -slot.min_y * mm_per_px;
    let y_bot = -slot.max_y * mm_per_px;
    let z = 0.14_f32;
    let p00 = rot90_ccw(x0, y_bot, cx, cy);
    let p10 = rot90_ccw(x1, y_bot, cx, cy);
    let p11 = rot90_ccw(x1, y_top, cx, cy);
    let p01 = rot90_ccw(x0, y_top, cx, cy);
    let u0 = slot.u0;
    let v0 = slot.v0;
    let u1 = slot.u1;
    let v1 = slot.v1;
    let verts = [
        LabelVertex {
            position: [p00[0], p00[1], z],
            uv: [u0, v1],
            color,
        },
        LabelVertex {
            position: [p10[0], p10[1], z],
            uv: [u1, v1],
            color,
        },
        LabelVertex {
            position: [p11[0], p11[1], z],
            uv: [u1, v0],
            color,
        },
        LabelVertex {
            position: [p00[0], p00[1], z],
            uv: [u0, v1],
            color,
        },
        LabelVertex {
            position: [p11[0], p11[1], z],
            uv: [u1, v0],
            color,
        },
        LabelVertex {
            position: [p01[0], p01[1], z],
            uv: [u0, v0],
            color,
        },
    ];
    out.extend_from_slice(&verts);
}

pub fn plate_labels(bed: &elysian_config::BedShape, color: [f32; 3]) -> Vec<LabelVertex> {
    let mut out = Vec::new();
    let (left, right) = bed.visible_only_rects();
    if let Some(rect) = left {
        out.extend(atlas().quads(rect, LEFT_ONLY_CAPTION, color));
    }
    if let Some(rect) = right {
        out.extend(atlas().quads(rect, RIGHT_ONLY_CAPTION, color));
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;
    use elysian_config::BedShape;

    #[test]
    fn atlas_contains_plate_letters() {
        let atlas = atlas();
        assert!(atlas.width >= 256);
        assert!(!atlas.pixels.iter().all(|p| *p == 0), "atlas is empty");
        for ch in LEFT_ONLY_CAPTION.chars().filter(|c| !c.is_whitespace()) {
            assert!(
                atlas.glyph(ch).is_some(),
                "missing glyph {ch} (Studio caption {LEFT_ONLY_CAPTION:?})"
            );
        }
    }

    #[test]
    fn rotated_caption_reads_front_to_back() {
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
            l_y < y_y - 1.0,
            "Studio caption runs along +Y, L (first) in front of Y: {l_y} vs {y_y}"
        );
        let l_x: f32 = verts[..6].iter().map(|v| v.position[0]).sum::<f32>() / 6.0;
        assert!(
            (l_x - 12.5).abs() < 8.0,
            "rotated letters sit in the strip, got x={l_x}"
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
        assert_eq!(
            verts.len(),
            6 * LEFT_ONLY_CAPTION
                .chars()
                .filter(|c| !c.is_whitespace())
                .count(),
            "expected Studio caption {LEFT_ONLY_CAPTION:?} with spaces as advances, got {}",
            verts.len()
        );
    }
}
