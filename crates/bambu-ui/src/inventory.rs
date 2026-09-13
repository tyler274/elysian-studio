//! Native Filament Manager: local `spools.json` + cloud GET/POST/PUT/DELETE.

use iced::widget::{button, column, row, scrollable, text, text_input};
use iced::{Element, Fill};

use bambu_protocol::FilamentSpool;

use crate::Message;

impl crate::App {
    pub(crate) fn inventory_page(&self) -> Element<'_, Message> {
        let filtered: Vec<(usize, &FilamentSpool)> = self
            .spools
            .iter()
            .enumerate()
            .filter(|(_, s)| {
                if self.spool_search.is_empty() {
                    return true;
                }
                let q = self.spool_search.to_ascii_lowercase();
                s.label().to_ascii_lowercase().contains(&q)
                    || s.filament_id.to_ascii_lowercase().contains(&q)
            })
            .collect();
        let mut list = column![
            text("Filament inventory").size(18),
            text("Local spools.json · cloud when Bearer is present (no plugin).").size(12),
            text_input("search", &self.spool_search).on_input(Message::InventorySearch),
            row![
                button("New").on_press(Message::InventoryNew),
                button("Save").on_press(Message::InventorySave),
                button("Delete").on_press(Message::InventoryDelete),
            ]
            .spacing(6),
            row![
                button("Pull").on_press(Message::InventoryPull),
                button("Push").on_press(Message::InventoryPush),
                button("Bind to slot").on_press(Message::BindSpoolToSlot),
            ]
            .spacing(6),
        ];
        for (i, spool) in filtered {
            let mark = if self.selected_spool == Some(i) {
                "●"
            } else {
                "○"
            };
            list = list.push(
                button(text(format!("{mark} {}", spool.label())).size(12))
                    .on_press(Message::InventorySelect(i)),
            );
        }
        list = list.push(self.inventory_editor());
        scrollable(list.spacing(8).padding(16).width(480))
            .height(Fill)
            .into()
    }

    fn inventory_editor(&self) -> Element<'_, Message> {
        let s = &self.draft_spool;
        column![
            text("Edit spool").size(14),
            text_input("brand", &s.brand).on_input(Message::SpoolBrand),
            text_input("material", &s.material_type).on_input(Message::SpoolMaterial),
            text_input("series / name", &s.series).on_input(Message::SpoolSeries),
            text_input("color #RRGGBB", &s.color_code).on_input(Message::SpoolColor),
            text_input("setting_id / filament_id", &s.filament_id)
                .on_input(Message::SpoolFilamentId),
            text_input("note", &s.note).on_input(Message::SpoolNote),
            text(format!(
                "remain {}% · {} g / {} g · cloud {}",
                s.remain_percent,
                s.net_weight,
                s.initial_weight,
                if s.cloud_synced { "yes" } else { "local" }
            ))
            .size(12),
        ]
        .spacing(6)
        .into()
    }
}
