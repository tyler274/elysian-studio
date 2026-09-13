//! Prepare-sidebar filament slots (system / user presets + colour).

use iced::widget::{button, column, container, pick_list, row, text, text_input};
use iced::{Background, Border, Color, Element};

use crate::{slot_colour_hex, FilamentSource, Message};

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
            text("Filament").size(16),
            button("+").on_press(Message::AddFilamentSlot),
            button("-").on_press(Message::RemoveFilamentSlot),
        ]
        .spacing(6)];
        for (i, slot) in self.filament_slots.iter().enumerate() {
            let selected = Some(filament_pick_label(slot.source, &slot.name));
            let swatch = colour_swatch(&slot.colour);
            let mark = if i == self.active_filament {
                "●"
            } else {
                "○"
            };
            slots = slots.push(
                column![
                    row![
                        button(text(format!("{mark} {}", i + 1)).size(12))
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
        for p in &self.filament_profiles {
            out.push(filament_pick_label(FilamentSource::System, &p.name));
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
}

pub(crate) fn filament_pick_label(source: FilamentSource, name: &str) -> String {
    match source {
        FilamentSource::System => format!("System · {name}"),
        FilamentSource::User => format!("User · {name}"),
        FilamentSource::Studio => format!("Studio · {name}"),
    }
}

pub(crate) fn parse_filament_pick(label: &str) -> Option<(FilamentSource, String)> {
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
