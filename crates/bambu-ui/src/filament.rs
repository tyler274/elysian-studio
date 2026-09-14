//! Prepare-sidebar filament slots (system / user presets + colour).

use iced::widget::{button, checkbox, column, container, pick_list, row, slider, text, text_input};
use iced::{Background, Border, Color, Element};

use bambu_config::FilamentMapMode;

use crate::{slot_colour_hex, FilamentPage, FilamentSource, Message};

const PALETTE: [&str; 8] = [
    "#FFFFFFFF",
    "#FF0000FF",
    "#00C853FF",
    "#2979FFFF",
    "#FFEA00FF",
    "#FF6D00FF",
    "#8E24AAFF",
    "#212121FF",
];

impl crate::App {
    pub(crate) fn filament_library(&self) -> Element<'_, Message> {
        let picks = self.filament_pick_labels();
        let mut slots = column![row![
        text("Project Filaments").size(16),
            button("+").on_press(Message::AddFilamentSlot),
            button("-").on_press(Message::RemoveFilamentSlot),
        ]
        .spacing(6)];
        for (i, slot) in self.filament_slots.iter().enumerate() {
            let selected = Some(slot_pick_label(slot));
            let swatch = colour_swatch(&slot.colour);
            let mark = if i == self.active_filament {
                "●"
            } else {
                "○"
            };
            let ams = match (slot.ams_id, slot.slot_id) {
                (Some(254), _) => "Ext".to_string(),
                (Some(a), Some(s)) => format!("A{a}S{s}"),
                _ => String::new(),
            };
            slots = slots.push(
                column![
                    row![
                        button(text(format!("{mark} {} {ams}", i + 1)).size(12))
                            .on_press(Message::SelectFilamentSlot(i)),
                        button(swatch).on_press(Message::CycleFilamentColour(i)),
                        pick_list(picks.clone(), selected, move |label| {
                            Message::FilamentSlotPreset { slot: i, label }
                        })
                        .placeholder("preset"),
                    ]
                    .spacing(6),
                    text_input("#RRGGBBAA", &slot.colour)
                        .on_input(move |colour| Message::FilamentSlotColour { slot: i, colour }),
                ]
                .spacing(4),
            );
        }
        slots = slots.push(
            text(format!(
                "{} {} · {}°C · {} slot(s)",
                self.settings.filament_vendor,
                self.settings.filament_type,
                self.settings.temperature_c,
                self.filament_slots.len().max(1)
            ))
            .size(12),
        );
        slots = slots.push(self.ams_chips());
        slots = slots.push(button("Sync AMS").on_press(Message::SyncAms));
        slots = slots.push(
            checkbox(self.show_bbl_presets)
                .label("Bambu system presets")
                .on_toggle(Message::ShowBblPresets),
        );
        slots = slots.push(
            text_input("catalog search", &self.catalog_query).on_input(Message::CatalogQuery),
        );
        slots = slots.push(self.filament_group_ui());
        slots = slots.push(self.filament_params());
        slots = slots.push(text("New user preset…").size(13));
        slots = slots.push(
            text_input("name (clone selected)", &self.user_preset_name)
                .on_input(Message::UserPresetName),
        );
        slots = slots.push(button("Save user preset").on_press(Message::SaveUserPreset));
        if self
            .filament_slots
            .get(self.active_filament)
            .is_some_and(|s| s.source == FilamentSource::User)
        {
            slots = slots.push(button("Delete user preset").on_press(Message::DeleteUserPreset));
        }
        slots.spacing(6).into()
    }

    pub(crate) fn filament_pick_labels(&self) -> Vec<String> {
        let mut out = Vec::new();
        for spool in &self.inventory.spools {
            if spool.archived {
                continue;
            }
            let filament = self.inventory.filament(&spool.filament_id);
            let vendor = filament
                .and_then(|f| self.inventory.vendor(&f.vendor_id))
                .map(|v| v.name.as_str())
                .unwrap_or("—");
            let name = filament.map(|f| f.name.as_str()).unwrap_or("spool");
            out.push(format!("Inventory · {} · {vendor} {name}", spool.id));
        }
        for sku in self.catalog.search(&self.catalog_query, 40) {
            out.push(format!(
                "Catalog · {} · {} {}",
                sku.external_id, sku.manufacturer, sku.name
            ));
        }
        if self.show_bbl_presets {
            for p in &self.filament_profiles {
                out.push(filament_pick_label(FilamentSource::System, &p.name));
            }
        }
        for p in &self.user_filaments {
            out.push(filament_pick_label(FilamentSource::User, &p.name));
        }
        for p in &self.studio_filaments {
            if self.user_filaments.iter().any(|u| u.name == p.name) {
                continue;
            }
            out.push(filament_pick_label(FilamentSource::Studio, &p.name));
        }
        out
    }

    pub(crate) fn filament_group_ui(&self) -> Element<'_, Message> {
        let nozzles = self.settings.nozzle_count();
        let mut col = column![
            text("Filament group").size(16),
            pick_list(
                vec![
                    FilamentMapMode::AutoForFlush,
                    FilamentMapMode::AutoForMatch,
                    FilamentMapMode::AutoForQuality,
                    FilamentMapMode::Manual,
                ],
                Some(self.settings.filament_map_mode),
                Message::FilamentMapMode
            ),
        ];
        if nozzles <= 1 {
            col = col.push(text("Single nozzle — map stays T1 (H2C dual shows 1/2).").size(11));
        }
        if self.settings.filament_map_mode == FilamentMapMode::Manual {
            for (i, mapped) in self.settings.filament_map.iter().enumerate() {
                col = col.push(
                    row![
                        text(format!("F{} → T{mapped}", i + 1)).size(12),
                        button("1").on_press(Message::AssignSlotExtruder {
                            slot: i,
                            extruder: 1
                        }),
                        button("2").on_press(Message::AssignSlotExtruder {
                            slot: i,
                            extruder: 2
                        }),
                    ]
                    .spacing(4),
                );
            }
        }
        col.spacing(6).into()
    }

    pub(crate) fn filament_params(&self) -> Element<'_, Message> {
        let pages = vec![
            FilamentPage::Filament,
            FilamentPage::Cooling,
            FilamentPage::Overrides,
            FilamentPage::Advanced,
            FilamentPage::Notes,
            FilamentPage::Multi,
        ];
        let mut col = column![
            text("Selected slot").size(16),
            pick_list(pages, Some(self.filament_page), Message::FilamentPage),
        ];
        col = col.push(match self.filament_page {
            FilamentPage::Filament => self.page_filament_basic(),
            FilamentPage::Cooling => self.page_cooling(),
            FilamentPage::Overrides => self.page_overrides(),
            FilamentPage::Advanced => self.page_advanced(),
            FilamentPage::Notes => self.page_notes(),
            FilamentPage::Multi => self.page_multi(),
        });
        col.spacing(6).into()
    }

    fn page_filament_basic(&self) -> Element<'_, Message> {
        column![
            text(format!(
                "id {} · {} {}",
                if self.settings.filament_id.is_empty() {
                    "—"
                } else {
                    self.settings.filament_id.as_str()
                },
                self.settings.filament_vendor,
                self.settings.filament_type
            ))
            .size(12),
            checkbox(self.settings.filament_soluble)
                .label("Soluble")
                .on_toggle(Message::ParamSoluble),
            checkbox(self.settings.filament_is_support)
                .label("Support filament")
                .on_toggle(Message::ParamSupport),
            checkbox(self.settings.filament_printable != 0)
                .label("Printable")
                .on_toggle(Message::ParamPrintable),
            checkbox(self.settings.enable_pressure_advance)
                .label("Pressure advance")
                .on_toggle(Message::ParamPaEnable),
            text(format!("K {:.3}", self.settings.pressure_advance)).size(12),
            slider(0.0..=0.1, self.settings.pressure_advance, Message::ParamPa).step(0.001),
            text(format!(
                "Range low {}°C · high {}°C",
                self.settings.nozzle_temperature_range_low,
                self.settings.nozzle_temperature_range_high
            ))
            .size(12),
            slider(
                0.0..=250.0,
                f64::from(self.settings.nozzle_temperature_range_low),
                Message::ParamRangeLow
            )
            .step(1.0),
            text(format!("Cost {:.2} / kg", self.settings.filament_cost)).size(12),
            slider(0.0..=80.0, self.settings.filament_cost, Message::ParamCost).step(0.5),
            checkbox(self.settings.filament_adaptive_volumetric_speed)
                .label("Adaptive volumetric")
                .on_toggle(Message::ParamAdaptiveVol),
            text(format!(
                "Prime volume {:.1} mm³",
                self.settings.filament_prime_volume
            ))
            .size(12),
            slider(
                0.0..=80.0,
                self.settings.filament_prime_volume,
                Message::ParamPrime
            )
            .step(0.5),
        ]
        .spacing(4)
        .into()
    }

    fn page_cooling(&self) -> Element<'_, Message> {
        column![
            text(format!("Fan min {}", self.settings.fan_min_speed)).size(12),
            slider(
                0.0..=100.0,
                f64::from(self.settings.fan_min_speed),
                Message::CoolingFanMin
            )
            .step(1.0),
            text(format!("Fan max {}", self.settings.fan_max_speed)).size(12),
            slider(
                0.0..=100.0,
                f64::from(self.settings.fan_max_speed),
                Message::CoolingFanMax
            )
            .step(1.0),
            text(format!(
                "Slow down layer time {:.0}s",
                self.settings.slow_down_layer_time_s
            ))
            .size(12),
            slider(
                0.0..=30.0,
                self.settings.slow_down_layer_time_s,
                Message::CoolingSlowdown
            )
            .step(1.0),
        ]
        .spacing(4)
        .into()
    }

    fn page_overrides(&self) -> Element<'_, Message> {
        column![
            text(format!(
                "Retract {:.2} mm",
                self.settings.retraction_length_mm
            ))
            .size(12),
            slider(
                0.0..=5.0,
                self.settings.retraction_length_mm,
                Message::ParamRetract
            )
            .step(0.05),
            checkbox(self.settings.wipe)
                .label("Wipe")
                .on_toggle(Message::ParamWipe),
        ]
        .spacing(4)
        .into()
    }

    fn page_advanced(&self) -> Element<'_, Message> {
        column![
            text("Start gcode").size(12),
            text_input("filament start", &self.settings.filament_start_gcode)
                .on_input(Message::ParamStartGcode),
            text("End gcode").size(12),
            text_input("filament end", &self.settings.filament_end_gcode)
                .on_input(Message::ParamEndGcode),
        ]
        .spacing(4)
        .into()
    }

    fn page_notes(&self) -> Element<'_, Message> {
        column![
            text("Notes").size(12),
            text_input("filament notes", &self.settings.filament_notes)
                .on_input(Message::ParamNotes),
        ]
        .spacing(4)
        .into()
    }

    fn page_multi(&self) -> Element<'_, Message> {
        column![
            text(format!(
                "Flush temp {}°C",
                self.settings.filament_flush_temp
            ))
            .size(12),
            slider(
                0.0..=280.0,
                f64::from(self.settings.filament_flush_temp.max(0) as u16),
                Message::ParamFlushTemp
            )
            .step(1.0),
            text(format!(
                "Flush temp fast {}°C",
                self.settings.filament_flush_temp_fast
            ))
            .size(12),
            slider(
                0.0..=280.0,
                f64::from(self.settings.filament_flush_temp_fast.max(0) as u16),
                Message::ParamFlushTempFast
            )
            .step(1.0),
            text(format!(
                "Flush volumetric {:.1}",
                self.settings.filament_flush_volumetric_speed
            ))
            .size(12),
            slider(
                0.0..=40.0,
                self.settings.filament_flush_volumetric_speed,
                Message::ParamFlushVol
            )
            .step(0.5),
            text(format!(
                "Ramming volumetric {:.1} (−1 = max)",
                self.settings.filament_ramming_volumetric_speed
            ))
            .size(12),
            slider(
                -1.0..=40.0,
                self.settings.filament_ramming_volumetric_speed,
                Message::ParamRammingVol
            )
            .step(0.5),
            text(format!(
                "Ramming travel {:.2}s",
                self.settings.filament_ramming_travel_time
            ))
            .size(12),
            slider(
                0.0..=5.0,
                self.settings.filament_ramming_travel_time,
                Message::ParamRammingTravel
            )
            .step(0.05),
            text(format!(
                "Precool {}°C",
                self.settings.filament_pre_cooling_temperature
            ))
            .size(12),
            slider(
                0.0..=280.0,
                f64::from(self.settings.filament_pre_cooling_temperature.max(0) as u16),
                Message::ParamPrecool
            )
            .step(1.0),
            checkbox(self.settings.long_retraction_when_ec)
                .label("Long retraction when EC")
                .on_toggle(Message::ParamLongEc),
        ]
        .spacing(4)
        .into()
    }
}

pub(crate) fn slot_pick_label(slot: &crate::FilamentSlot) -> String {
    match slot.source {
        FilamentSource::Inventory if !slot.spool_id.is_empty() => {
            format!("Inventory · {} · {}", slot.spool_id, slot.name)
        }
        FilamentSource::Catalog if !slot.external_id.is_empty() => {
            format!("Catalog · {} · {}", slot.external_id, slot.name)
        }
        _ => filament_pick_label(slot.source, &slot.name),
    }
}

pub(crate) fn filament_pick_label(source: FilamentSource, name: &str) -> String {
    match source {
        FilamentSource::System => format!("System · {name}"),
        FilamentSource::User => format!("User · {name}"),
        FilamentSource::Studio => format!("Studio · {name}"),
        FilamentSource::Catalog => format!("Catalog · {name}"),
        FilamentSource::Inventory => format!("Inventory · {name}"),
    }
}

pub(crate) fn parse_filament_pick(label: &str) -> Option<(FilamentSource, String)> {
    if let Some(rest) = label.strip_prefix("Inventory · ") {
        let id = rest.split(" · ").next().unwrap_or(rest).to_string();
        return Some((FilamentSource::Inventory, id));
    }
    if let Some(rest) = label.strip_prefix("Catalog · ") {
        let id = rest.split(" · ").next().unwrap_or(rest).to_string();
        return Some((FilamentSource::Catalog, id));
    }
    if let Some(name) = label.strip_prefix("System · ") {
        return Some((FilamentSource::System, name.to_string()));
    }
    if let Some(name) = label.strip_prefix("User · ") {
        return Some((FilamentSource::User, name.to_string()));
    }
    if let Some(name) = label.strip_prefix("Studio · ") {
        return Some((FilamentSource::Studio, name.to_string()));
    }
    None
}

pub(crate) fn next_palette_colour(current: &str) -> String {
    let hex = slot_colour_hex(current);
    let idx = PALETTE.iter().position(|c| *c == hex.as_str()).unwrap_or(0);
    PALETTE[(idx + 1) % PALETTE.len()].to_string()
}

fn colour_swatch(hex: &str) -> Element<'static, Message> {
    let parsed = slot_colour_hex(hex);
    let r = u8::from_str_radix(parsed.get(1..3).unwrap_or("FF"), 16).unwrap_or(255);
    let g = u8::from_str_radix(parsed.get(3..5).unwrap_or("FF"), 16).unwrap_or(255);
    let b = u8::from_str_radix(parsed.get(5..7).unwrap_or("FF"), 16).unwrap_or(255);
    container(text(" ").size(10))
        .width(22)
        .height(18)
        .style(move |_| container::Style {
            background: Some(Background::Color(Color::from_rgb8(r, g, b))),
            border: Border {
                color: Color::from_rgb(0.3, 0.3, 0.35),
                width: 1.0,
                radius: 3.0.into(),
            },
            ..container::Style::default()
        })
        .into()
}
