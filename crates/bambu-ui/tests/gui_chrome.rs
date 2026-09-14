//! Headless Prepare/Preview chrome goldens: JSON `GuiSnapshot` plus GPU-tolerant PNGs.

use std::fs;
use std::path::PathBuf;

use bambu_config::SeamPosition;
use bambu_ui::theme;
use bambu_ui::{
    decode_png, encode_png, header_has_fill, png_delta, sidebar_is_width, write_png, App,
    GuiSnapshot, Message, ProcessTab, Workspace, WINDOW_SIZE,
};

const RMSE_MAX: f64 = 12.0;
const MAX_DELTA: u8 = 48;

fn golden_dir() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("tests/gui/golden")
}

fn update_golden() -> bool {
    matches!(
        std::env::var("UPDATE_GUI_GOLDEN").as_deref(),
        Ok("1") | Ok("true")
    )
}

fn drive(app: &mut App, messages: impl IntoIterator<Item = Message>) {
    for message in messages {
        let _ = app.update(message);
    }
}

fn capture(app: &App, name: &str) -> (GuiSnapshot, Option<(u32, u32, Vec<u8>)>) {
    let snap = app.snapshot();
    let png = app
        .screenshot_rgba()
        .map(|rgba| (WINDOW_SIZE.width as u32, WINDOW_SIZE.height as u32, rgba));
    if let Some((w, h, ref rgba)) = png {
        let dir = std::env::temp_dir().join("bambu-ui-gui");
        let _ = fs::create_dir_all(&dir);
        let _ = write_png(&dir.join(format!("{name}.png")), rgba, w, h);
        eprintln!("wrote {}/{name}.png", dir.display());
    }
    (snap, png)
}

fn assert_json(name: &str, snap: &GuiSnapshot) {
    let dir = golden_dir();
    let path = dir.join(format!("{name}.json"));
    let pretty = serde_json::to_string_pretty(snap).expect("snapshot json");
    if update_golden() {
        fs::create_dir_all(&dir).expect("golden dir");
        fs::write(&path, pretty.as_bytes()).expect("write json golden");
        return;
    }
    let expected = fs::read_to_string(&path).unwrap_or_else(|_| {
        panic!(
            "missing GUI JSON golden {} — rerun with UPDATE_GUI_GOLDEN=1",
            path.display()
        )
    });
    let expected_val: serde_json::Value =
        serde_json::from_str(&expected).expect("parse golden json");
    let actual_val: serde_json::Value = serde_json::from_str(&pretty).expect("parse actual json");
    assert_eq!(
        expected_val, actual_val,
        "GuiSnapshot mismatch for {name}\n--- golden ---\n{expected}\n--- actual ---\n{pretty}"
    );
}

fn assert_png(name: &str, rgba: &[u8], width: u32, height: u32) {
    let dir = golden_dir();
    let path = dir.join(format!("{name}.png"));
    if update_golden() {
        fs::create_dir_all(&dir).expect("golden dir");
        write_png(&path, rgba, width, height).expect("write png golden");
        return;
    }
    let bytes = fs::read(&path).unwrap_or_else(|_| {
        panic!(
            "missing GUI PNG golden {} — rerun with UPDATE_GUI_GOLDEN=1",
            path.display()
        )
    });
    let (gw, gh, golden) = decode_png(&bytes).expect("decode golden png");
    assert_eq!((gw, gh), (width, height), "{name} png size");
    let (rmse, max) = png_delta(rgba, &golden);
    assert!(
        rmse <= RMSE_MAX && max <= MAX_DELTA,
        "{name} png delta rmse={rmse:.3} max={max} (limits {RMSE_MAX}/{MAX_DELTA})"
    );
}

fn assert_structure(snap: &GuiSnapshot, rgba: Option<&[u8]>, prepare: bool) {
    for needle in [
        "Home",
        "Prepare",
        "Preview",
        "Slice plate",
        "Print plate",
        "File",
        "AMS",
        "Printer",
        "Filament",
        "Process",
        "Quality",
        "Seam",
        "Iso",
        "Top",
        "Front",
    ] {
        assert!(
            snap.labels.iter().any(|s| s == needle),
            "missing label {needle}"
        );
    }
    assert_eq!(snap.sidebar_width, 300);
    assert!(!snap.sidebar_collapsed);
    assert!(snap.printer_open && snap.filament_open && snap.process_open);
    assert_eq!(snap.window, [1200, 800]);
    if prepare {
        assert_eq!(snap.workspace, "prepare");
        assert_eq!(snap.active_tab_fill, "#00AE42");
    } else {
        assert_eq!(snap.workspace, "preview");
        assert_eq!(snap.active_tab_fill, "#1A9B8A");
        assert!(snap.fps_visible);
    }
    if let Some(px) = rgba {
        let fill = if prepare {
            theme::PREPARE
        } else {
            theme::PREVIEW
        };
        assert!(
            header_has_fill(px, 1200, fill),
            "header tab fill {:?} missing",
            fill
        );
        assert!(
            sidebar_is_width(px, 1200, 800),
            "sidebar width probe failed"
        );
    }
}

fn maybe_png<'a>(name: &str, png: &'a Option<(u32, u32, Vec<u8>)>) -> Option<&'a [u8]> {
    png.as_ref().map(|(w, h, rgba)| {
        assert_png(name, rgba, *w, *h);
        rgba.as_slice()
    })
}

#[test]
fn gui_chrome_prepare_preview_goldens() {
    let mut app = App::new_for_gui_test();

    let (snap, png) = capture(&app, "prepare_default");
    assert_json("prepare_default", &snap);
    let rgba = maybe_png("prepare_default", &png);
    assert_structure(&snap, rgba, true);
    assert_eq!(snap.quality_tab, "quality");

    app.seed_preview_gcode_placeholder();
    drive(&mut app, [Message::Workspace(Workspace::Preview)]);
    let (snap, png) = capture(&app, "preview_empty");
    assert_json("preview_empty", &snap);
    let rgba = maybe_png("preview_empty", &png);
    assert_structure(&snap, rgba, false);
    assert_eq!(snap.plate_badge, "01");

    app.slice_cpu_blocking();
    drive(&mut app, [Message::PreviewLayer(10_000)]);
    let (snap, png) = capture(&app, "preview_sliced");
    assert_json("preview_sliced", &snap);
    let rgba = maybe_png("preview_sliced", &png);
    assert_structure(&snap, rgba, false);

    drive(&mut app, [Message::ProcessObjects(true)]);
    assert!(app.snapshot().process_objects);
    drive(&mut app, [Message::ProcessObjects(false)]);
    assert!(!app.snapshot().process_objects);

    drive(
        &mut app,
        [
            Message::Workspace(Workspace::Prepare),
            Message::ProcessTab(ProcessTab::Quality),
            Message::ProcessSearch("Standard".into()),
            Message::Seam(SeamPosition::Rear),
            Message::PreciseOuterWall(true),
            Message::PlateNext,
        ],
    );
    let (snap, png) = capture(&app, "prepare_quality_seam");
    assert_json("prepare_quality_seam", &snap);
    let rgba = maybe_png("prepare_quality_seam", &png);
    assert_structure(&snap, rgba, true);
    assert_eq!(snap.quality_tab, "quality");
    assert_eq!(snap.seam, "rear");
    assert_eq!(snap.process_search, "Standard");

    drive(&mut app, [Message::CollapseSidebar]);
    let collapsed = app.snapshot();
    assert!(collapsed.sidebar_collapsed);
    assert_eq!(collapsed.sidebar_width, 0);
    drive(&mut app, [Message::CollapseSidebar]);
    assert!(!app.snapshot().sidebar_collapsed);
    assert_eq!(app.snapshot().sidebar_width, 300);

    drive(
        &mut app,
        [
            Message::ToggleFilamentSection,
            Message::TogglePrinterSection,
            Message::ToggleProcessSection,
        ],
    );
    assert_eq!(app.snapshot().active_tab_fill, "#00AE42");
    if let Some(rgba) = app.screenshot_rgba() {
        assert!(
            header_has_fill(&rgba, 1200, theme::PREPARE),
            "collapsing sidebar groups must keep Prepare tab fill"
        );
        assert!(
            sidebar_is_width(&rgba, 1200, 800),
            "collapsing sidebar groups must keep Studio sidebar chrome"
        );
    }
    drive(
        &mut app,
        [
            Message::ToggleFilamentSection,
            Message::TogglePrinterSection,
            Message::ToggleProcessSection,
        ],
    );
    assert!(
        app.snapshot().printer_open && app.snapshot().filament_open && app.snapshot().process_open
    );

    if png.is_none() {
        eprintln!("skipping GUI PNG goldens (no wgpu adapter)");
    }
}

#[test]
fn gui_snapshot_labels_cover_reference_chrome() {
    let app = App::new_for_gui_test();
    let snap = app.snapshot();
    for needle in [
        "Home",
        "Prepare",
        "Preview",
        "Slice plate",
        "Print plate",
        "File",
        "AMS",
        "Printer",
        "Filament",
        "Process",
        "Quality",
        "Seam",
        "Iso",
        "Top",
        "Front",
    ] {
        assert!(snap.labels.iter().any(|s| s == needle), "{needle}");
    }
    assert_eq!(snap.sidebar_width, 300);
    assert!(!snap.sidebar_collapsed);
    assert!(snap.printer_open && snap.filament_open && snap.process_open);
}

#[test]
fn device_page_headless_renders() {
    let mut app = App::new_for_gui_test();
    drive(&mut app, [Message::Workspace(Workspace::Device)]);
    let snap = app.snapshot();
    assert_eq!(snap.workspace, "device");
    assert_eq!(snap.active_tab_fill, "#00AE42");
    if let Some(rgba) = app.screenshot_rgba() {
        assert_eq!(rgba.len(), 1200 * 800 * 4);
        assert!(
            header_has_fill(&rgba, 1200, theme::PREPARE),
            "Device header tab fill missing"
        );
    }
}

#[test]
fn gui_chrome_device_home_filament_goldens() {
    let mut app = App::new_for_gui_test();
    app.seed_device_monitor();
    drive(&mut app, [Message::Workspace(Workspace::Device)]);
    let (snap, png) = capture(&app, "device_seeded");
    assert_json("device_seeded", &snap);
    let rgba = maybe_png("device_seeded", &png);
    assert_eq!(snap.workspace, "device");
    assert_eq!(snap.active_tab_fill, "#00AE42");
    if let Some(px) = rgba {
        assert!(header_has_fill(px, 1200, theme::PREPARE));
    }

    drive(&mut app, [Message::Workspace(Workspace::Home)]);
    let (snap, png) = capture(&app, "home_default");
    assert_json("home_default", &snap);
    let rgba = maybe_png("home_default", &png);
    assert_eq!(snap.workspace, "home");
    if let Some(px) = rgba {
        assert!(header_has_fill(px, 1200, theme::PREPARE));
    }

    drive(&mut app, [Message::Workspace(Workspace::Filament)]);
    let (snap, png) = capture(&app, "filament_manager");
    assert_json("filament_manager", &snap);
    let rgba = maybe_png("filament_manager", &png);
    assert_eq!(snap.workspace, "filament");
    if let Some(px) = rgba {
        assert!(header_has_fill(px, 1200, theme::PREPARE));
    }
}

#[test]
fn gui_png_helpers_roundtrip() {
    let rgba = vec![10u8, 20, 30, 255, 1, 2, 3, 255];
    let encoded = encode_png(&rgba, 2, 1).expect("encode");
    let (w, h, out) = decode_png(&encoded).expect("decode");
    assert_eq!((w, h), (2, 1));
    assert_eq!(out, rgba);
    let (rmse, max) = png_delta(&rgba, &out);
    assert_eq!((rmse, max), (0.0, 0));
}
