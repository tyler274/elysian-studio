//! Printable-area `XxY` lists used by Bambu process / machine JSON.

pub(super) fn format_xy_list(pts: &[(f64, f64)]) -> String {
    pts.iter()
        .map(|(x, y)| format!("{x}x{y}"))
        .collect::<Vec<_>>()
        .join(",")
}
