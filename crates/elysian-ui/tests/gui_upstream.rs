//! Full-window PNG RMSE of iced chrome vs stored Bambu Studio / Orca / Prusa captures.
//!
//! wxWidgets vs iced is not pixel-identical. Gate on RMSE only (max-channel is always ~255).
//! Missing refs skip unless `BAMBU_STUDIO_REQUIRE_ORACLE=1`. Refresh refs with
//! `scripts/capture-upstream-gui.sh` / `UPDATE_GUI_UPSTREAM=1`.

use std::fs;
use std::path::PathBuf;

use elysian_ui::{
    decode_png, png_delta, scale_rgba, write_png, App, Message, Workspace, WINDOW_SIZE,
};

const TARGET_W: u32 = 1200;
const TARGET_H: u32 = 800;
/// Loose full-window RMSE across toolkits. Tuned on Studio Home / Orca plater
/// captures (~35) so a blank or light-theme iced frame fails (~90+).
const UPSTREAM_RMSE_MAX: f64 = 90.0;
/// Empty Xvfb grabs are near-black; real Studio/Orca chrome is not.
const UPSTREAM_MIN_LUMA: f64 = 12.0;

fn require_oracle() -> bool {
    matches!(
        std::env::var("BAMBU_STUDIO_REQUIRE_ORACLE").as_deref(),
        Ok("1") | Ok("true") | Ok("TRUE") | Ok("yes")
    )
}

fn upstream_dir() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("tests/gui/upstream")
}

fn mean_luma(rgba: &[u8]) -> f64 {
    let n = (rgba.len() / 4).max(1) as f64;
    let mut sum = 0.0;
    for px in rgba.chunks_exact(4) {
        sum += 0.2126 * f64::from(px[0]) + 0.7152 * f64::from(px[1]) + 0.0722 * f64::from(px[2]);
    }
    sum / n
}

fn load_scaled(path: &PathBuf) -> Option<(u32, u32, Vec<u8>)> {
    let bytes = fs::read(path).ok()?;
    let (w, h, rgba) = decode_png(&bytes).ok()?;
    if w == TARGET_W && h == TARGET_H {
        return Some((w, h, rgba));
    }
    Some((
        TARGET_W,
        TARGET_H,
        scale_rgba(&rgba, w, h, TARGET_W, TARGET_H),
    ))
}

fn iced_frame(workspace: Workspace, seed_device: bool) -> Option<Vec<u8>> {
    let mut app = App::new_for_gui_test();
    if seed_device {
        app.seed_device_monitor();
    }
    if workspace == Workspace::Preview {
        app.seed_preview_gcode_placeholder();
    }
    let _ = app.update(Message::Workspace(workspace));
    app.screenshot_rgba()
}

fn compare_or_skip(iced: &[u8], rel: &str) {
    let path = upstream_dir().join(rel);
    if !path.is_file() {
        if require_oracle() {
            panic!(
                "BAMBU_STUDIO_REQUIRE_ORACLE=1 but missing upstream PNG {}",
                path.display()
            );
        }
        eprintln!("skipping upstream diff (missing {})", path.display());
        return;
    }
    let (_w, _h, up) = load_scaled(&path).unwrap_or_else(|| panic!("decode {}", path.display()));
    assert_eq!(iced.len(), (TARGET_W * TARGET_H * 4) as usize);
    assert_eq!(up.len(), iced.len());
    let luma = mean_luma(&up);
    assert!(
        luma >= UPSTREAM_MIN_LUMA,
        "{} looks like an empty/black Xvfb grab (mean luma {luma:.1})",
        path.display()
    );
    let (rmse, max) = png_delta(iced, &up);
    let dir = std::env::temp_dir().join("elysian-ui-gui");
    let _ = fs::create_dir_all(&dir);
    let stem = rel.replace('/', "_").replace(".png", "");
    let _ = write_png(
        &dir.join(format!("upstream_{stem}.png")),
        &up,
        TARGET_W,
        TARGET_H,
    );
    eprintln!("{rel} rmse={rmse:.3} max={max} (limit {UPSTREAM_RMSE_MAX})");
    assert!(
        rmse <= UPSTREAM_RMSE_MAX,
        "{rel} full-window RMSE {rmse:.3} exceeds {UPSTREAM_RMSE_MAX} (max-channel {max})"
    );
}

fn assert_pairs(iced: &[u8], rels: &[&str]) {
    for rel in rels {
        compare_or_skip(iced, rel);
    }
}

#[test]
fn gui_upstream_prepare_plater() {
    let Some(rgba) = iced_frame(Workspace::Prepare, false) else {
        eprintln!("skipping upstream diffs (no wgpu adapter)");
        return;
    };
    assert_eq!(
        rgba.len(),
        (WINDOW_SIZE.width as u32 * WINDOW_SIZE.height as u32 * 4) as usize
    );
    assert_pairs(
        &rgba,
        &["bambu/prepare.png", "orca/prepare.png", "prusa/prepare.png"],
    );
}

#[test]
fn gui_upstream_preview() {
    let Some(rgba) = iced_frame(Workspace::Preview, false) else {
        eprintln!("skipping upstream diffs (no wgpu adapter)");
        return;
    };
    assert_pairs(&rgba, &["bambu/preview.png", "orca/preview.png"]);
}

#[test]
fn gui_upstream_device() {
    let Some(rgba) = iced_frame(Workspace::Device, true) else {
        eprintln!("skipping upstream diffs (no wgpu adapter)");
        return;
    };
    assert_pairs(&rgba, &["bambu/device.png"]);
}

#[test]
fn gui_upstream_home() {
    let Some(rgba) = iced_frame(Workspace::Home, false) else {
        eprintln!("skipping upstream diffs (no wgpu adapter)");
        return;
    };
    assert_pairs(&rgba, &["bambu/home.png"]);
}

#[test]
fn gui_upstream_filament() {
    let Some(rgba) = iced_frame(Workspace::Filament, false) else {
        eprintln!("skipping upstream diffs (no wgpu adapter)");
        return;
    };
    assert_pairs(&rgba, &["bambu/filament.png"]);
}
