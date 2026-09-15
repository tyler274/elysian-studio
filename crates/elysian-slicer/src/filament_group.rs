//! C++ `FilamentGroup`: assign each project filament to a 1-based extruder.
//!
//! Flush enumerates partitions when N is small; match maps AMS trays by
//! `filament_id` then colour/type. Cube stays `filament_map = [1]` because
//! `filament_count == 1`.

use elysian_config::{FilamentMapMode, SliceSettings};

#[derive(Debug, Clone, Default)]
pub struct GroupSlot {
    pub filament_id: String,
    pub colour: String,
    pub filament_type: String,
    pub is_support: bool,
}

#[derive(Debug, Clone, Default)]
pub struct GroupTray {
    pub ams_id: u8,
    pub tray_info_idx: String,
    pub filament_type: String,
    pub color: String,
}

/// Rewrite `filament_map` for auto modes when there is more than one filament
/// and more than one nozzle. Manual is left untouched.
pub fn apply_filament_group(
    settings: &mut SliceSettings,
    slots: &[GroupSlot],
    trays: &[GroupTray],
) {
    let n = settings.filament_count.max(slots.len()).max(1);
    let nozzles = settings.nozzle_count().max(1);
    if n <= 1 || settings.filament_map_mode == FilamentMapMode::Manual {
        if settings.filament_map.len() != n {
            settings.filament_map = vec![1; n];
        }
        return;
    }
    if nozzles <= 1 {
        settings.filament_map = vec![1; n];
        return;
    }
    let map = match settings.filament_map_mode {
        FilamentMapMode::AutoForMatch => match_map(n, nozzles, slots, trays),
        FilamentMapMode::AutoForQuality => {
            let mut map = flush_map(n, nozzles, &settings.flush_volumes_mm3);
            bias_support(&mut map, slots, nozzles);
            map
        }
        FilamentMapMode::AutoForFlush => flush_map(n, nozzles, &settings.flush_volumes_mm3),
        FilamentMapMode::Manual => settings.filament_map.clone(),
    };
    settings.filament_map = map;
}

pub fn compute_filament_map(
    mode: FilamentMapMode,
    n: usize,
    nozzles: usize,
    flush_volumes_mm3: &[f64],
    slots: &[GroupSlot],
    trays: &[GroupTray],
) -> Vec<i32> {
    let n = n.max(1);
    let nozzles = nozzles.max(1);
    if n <= 1 || nozzles <= 1 {
        return vec![1; n];
    }
    match mode {
        FilamentMapMode::Manual => vec![1; n],
        FilamentMapMode::AutoForMatch => match_map(n, nozzles, slots, trays),
        FilamentMapMode::AutoForQuality => {
            let mut map = flush_map(n, nozzles, flush_volumes_mm3);
            bias_support(&mut map, slots, nozzles);
            map
        }
        FilamentMapMode::AutoForFlush => flush_map(n, nozzles, flush_volumes_mm3),
    }
}

fn flush_volume(matrix: &[f64], n: usize, from: usize, to: usize) -> f64 {
    if from == to {
        return 0.0;
    }
    let idx = from * n + to;
    matrix
        .get(idx)
        .copied()
        .filter(|v| *v > 0.0)
        .unwrap_or(800.0)
}

/// Minimize same-nozzle flush (pairs that share an extruder pay their matrix cost).
fn flush_map(n: usize, nozzles: usize, matrix: &[f64]) -> Vec<i32> {
    if n > 12 || nozzles != 2 {
        return greedy_flush(n, nozzles, matrix);
    }
    let limit = 1usize << n;
    let mut best = vec![1i32; n];
    let mut best_cost = f64::INFINITY;
    for bits in 0..limit {
        let map: Vec<i32> = (0..n).map(|i| ((bits >> i) & 1) as i32 + 1).collect();
        let cost = assignment_cost(&map, n, matrix);
        if cost < best_cost {
            best_cost = cost;
            best = map;
        }
    }
    best
}

fn greedy_flush(n: usize, nozzles: usize, matrix: &[f64]) -> Vec<i32> {
    let _ = matrix;
    (0..n).map(|i| ((i % nozzles) as i32) + 1).collect()
}

fn assignment_cost(map: &[i32], n: usize, matrix: &[f64]) -> f64 {
    let mut cost = 0.0;
    for i in 0..n {
        for j in 0..n {
            if i == j || map[i] != map[j] {
                continue;
            }
            cost += flush_volume(matrix, n, i, j);
        }
    }
    cost
}

fn match_map(n: usize, nozzles: usize, slots: &[GroupSlot], trays: &[GroupTray]) -> Vec<i32> {
    let mut map = vec![1i32; n];
    for (i, slot) in slots.iter().enumerate().take(n) {
        if let Some(tray) = trays
            .iter()
            .find(|t| !slot.filament_id.is_empty() && t.tray_info_idx == slot.filament_id)
        {
            map[i] = tray_nozzle(tray.ams_id, nozzles);
            continue;
        }
        if let Some(tray) = trays.iter().find(|t| colours_close(&t.color, &slot.colour)) {
            map[i] = tray_nozzle(tray.ams_id, nozzles);
            continue;
        }
        if let Some(tray) = trays.iter().find(|t| {
            !slot.filament_type.is_empty()
                && t.filament_type.eq_ignore_ascii_case(&slot.filament_type)
        }) {
            map[i] = tray_nozzle(tray.ams_id, nozzles);
            continue;
        }
        map[i] = ((i % nozzles) as i32) + 1;
    }
    map
}

fn tray_nozzle(ams_id: u8, nozzles: usize) -> i32 {
    if ams_id == 254 {
        return 1;
    }
    ((ams_id as usize) % nozzles) as i32 + 1
}

fn colours_close(a: &str, b: &str) -> bool {
    let na = normalize_hex(a);
    let nb = normalize_hex(b);
    !na.is_empty() && na == nb
}

fn normalize_hex(raw: &str) -> String {
    raw.trim()
        .trim_start_matches('#')
        .chars()
        .take(6)
        .map(|c| c.to_ascii_uppercase())
        .collect()
}

fn bias_support(map: &mut [i32], slots: &[GroupSlot], nozzles: usize) {
    let support_nozzle = nozzles.max(1) as i32;
    for (i, slot) in slots.iter().enumerate() {
        if slot.is_support {
            if let Some(m) = map.get_mut(i) {
                *m = support_nozzle;
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn single_filament_stays_left_nozzle() {
        let map = compute_filament_map(FilamentMapMode::AutoForFlush, 1, 2, &[], &[], &[]);
        assert_eq!(map, vec![1]);
    }

    #[test]
    fn flush_splits_expensive_pair() {
        // 2×2: flushing 0→1 costs 2000, 1→0 costs 2000.
        let matrix = vec![0.0, 2000.0, 2000.0, 0.0];
        let map = compute_filament_map(FilamentMapMode::AutoForFlush, 2, 2, &matrix, &[], &[]);
        assert_eq!(map.len(), 2);
        assert_ne!(
            map[0], map[1],
            "high-flush pair should split nozzles: {map:?}"
        );
    }

    #[test]
    fn flush_four_filaments_enumerates() {
        let n = 4;
        let mut matrix = vec![800.0; n * n];
        for i in 0..n {
            matrix[i * n + i] = 0.0;
        }
        let map = compute_filament_map(FilamentMapMode::AutoForFlush, n, 2, &matrix, &[], &[]);
        assert_eq!(map.len(), 4);
        assert!(map.iter().all(|e| *e == 1 || *e == 2));
    }

    #[test]
    fn match_uses_tray_info_idx() {
        let slots = vec![
            GroupSlot {
                filament_id: "GFA00".into(),
                colour: "#FF0000".into(),
                filament_type: "PLA".into(),
                is_support: false,
            },
            GroupSlot {
                filament_id: "GFS00".into(),
                colour: "#FFFFFF".into(),
                filament_type: "PLA-S".into(),
                is_support: true,
            },
        ];
        let trays = vec![
            GroupTray {
                ams_id: 0,
                tray_info_idx: "GFA00".into(),
                filament_type: "PLA".into(),
                color: "#FF0000".into(),
            },
            GroupTray {
                ams_id: 1,
                tray_info_idx: "GFS00".into(),
                filament_type: "PLA-S".into(),
                color: "#FFFFFF".into(),
            },
        ];
        let map = compute_filament_map(FilamentMapMode::AutoForMatch, 2, 2, &[], &slots, &trays);
        assert_eq!(map, vec![1, 2]);
    }

    #[test]
    fn quality_biases_support_to_last_nozzle() {
        let slots = vec![
            GroupSlot {
                is_support: false,
                ..GroupSlot::default()
            },
            GroupSlot {
                is_support: true,
                ..GroupSlot::default()
            },
        ];
        let map = compute_filament_map(
            FilamentMapMode::AutoForQuality,
            2,
            2,
            &[0.0, 10.0, 10.0, 0.0],
            &slots,
            &[],
        );
        assert_eq!(map[1], 2);
    }

    #[test]
    fn apply_skips_when_count_is_one() {
        let mut s = SliceSettings::default();
        s.filament_count = 1;
        s.nozzle_diameters_mm = vec![0.4, 0.4];
        s.filament_map_mode = FilamentMapMode::AutoForFlush;
        apply_filament_group(&mut s, &[], &[]);
        assert_eq!(s.filament_map, vec![1]);
    }
}
