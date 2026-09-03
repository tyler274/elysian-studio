//! Shared 3MF XML helpers (attributes, units, 4×3 transforms).

use glam::{Mat4, Vec4};

pub(crate) const CORE_NS: &str = "http://schemas.microsoft.com/3dmanufacturing/core/2015/02";

/// 3MF 4×3 matrix (12 numbers) into a column-major [`Mat4`], matching C++
/// `get_transform_from_3mf_specs_string`.
pub(super) fn parse_transform(s: &str) -> Mat4 {
    let nums: Vec<f32> = s
        .split_whitespace()
        .filter_map(|t| t.parse().ok())
        .collect();
    if nums.len() != 12 {
        return Mat4::IDENTITY;
    }
    Mat4::from_cols(
        Vec4::new(nums[0], nums[1], nums[2], 0.0),
        Vec4::new(nums[3], nums[4], nums[5], 0.0),
        Vec4::new(nums[6], nums[7], nums[8], 0.0),
        Vec4::new(nums[9], nums[10], nums[11], 1.0),
    )
}

pub(super) fn unit_factor(unit: &str) -> f32 {
    match unit {
        "micron" => 0.001,
        "centimeter" => 10.0,
        "inch" => 25.4,
        "foot" => 304.8,
        "meter" => 1000.0,
        _ => 1.0,
    }
}

pub(super) fn attr<'a>(e: &'a quick_xml::events::BytesStart<'a>, key: &[u8]) -> Option<String> {
    e.try_get_attribute(key)
        .ok()
        .flatten()
        .and_then(|a| String::from_utf8(a.value.into_owned()).ok())
}

pub(super) fn attr_f32(e: &quick_xml::events::BytesStart<'_>, key: &[u8]) -> f32 {
    attr(e, key).and_then(|s| s.parse().ok()).unwrap_or(0.0)
}

pub(super) fn attr_u32(e: &quick_xml::events::BytesStart<'_>, key: &[u8]) -> u32 {
    attr(e, key).and_then(|s| s.parse().ok()).unwrap_or(0)
}

pub(super) fn xml_escape(s: &str) -> String {
    s.replace('&', "&amp;")
        .replace('<', "&lt;")
        .replace('>', "&gt;")
        .replace('"', "&quot;")
}
