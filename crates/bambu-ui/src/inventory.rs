//! Native Filament Manager: Spoolman-style inventory + SpoolmanDB catalog.

use iced::widget::{button, checkbox, column, row, scrollable, text, text_input};
use iced::{Element, Fill};

use bambu_protocol::{remain_percent, remaining_weight, InventorySpool};

use crate::Message;

impl crate::App {
    pub(crate) fn inventory_page(&self) -> Element<'_, Message> {
        let q = self.spool_search.to_ascii_lowercase();
        let filtered: Vec<(usize, &InventorySpool)> = self
            .inventory
            .spools
            .iter()
            .enumerate()
            .filter(|(_, s)| {
                if s.archived && !self.show_archived {
                    return false;
                }
                if q.is_empty() {
                    return true;
                }
                let flat = self.inventory.flatten(s);
                flat.label().to_ascii_lowercase().contains(&q)
                    || s.location.to_ascii_lowercase().contains(&q)
                    || s.id.to_ascii_lowercase().contains(&q)
            })
            .collect();
        let catalog_hits = self.catalog.search(&self.catalog_query, 24);
        let mut list = column![
            text("Filament inventory").size(18),
            text("Spoolman-style vendor → filament → spool · SpoolmanDB catalog default.").size(12),
            text_input("search spools", &self.spool_search).on_input(Message::InventorySearch),
            checkbox(self.show_archived)
                .label("Show archived")
                .on_toggle(Message::ShowArchived),
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
            text("Add from catalog").size(14),
            text_input("vendor / material / color", &self.catalog_query)
                .on_input(Message::CatalogQuery),
        ];
        for sku in catalog_hits {
            let id = sku.external_id.clone();
            list = list.push(
                button(
                    text(format!(
                        "+ {} {} · {}",
                        sku.manufacturer, sku.name, sku.material
                    ))
                    .size(11),
                )
                .on_press(Message::CatalogAdd(id)),
            );
        }
        list = list.push(text("Spools").size(14));
        for (i, spool) in filtered {
            let filament = self.inventory.filament(&spool.filament_id);
            let pct = remain_percent(spool, filament);
            let remain = remaining_weight(spool, filament);
            let name = filament.map(|f| f.name.as_str()).unwrap_or("spool");
            let vendor = filament
                .and_then(|f| self.inventory.vendor(&f.vendor_id))
                .map(|v| v.name.as_str())
                .unwrap_or("—");
            let mark = if self.selected_spool == Some(i) {
                "●"
            } else {
                "○"
            };
            let arch = if spool.archived { " [arch]" } else { "" };
            list = list.push(
                button(
                    text(format!(
                        "{mark} {vendor} {name} · {remain:.0}g ({pct}%){arch}"
                    ))
                    .size(12),
                )
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
            text_input("location (e.g. A0S1)", &self.draft_location)
                .on_input(Message::SpoolLocation),
            text_input("note", &s.note).on_input(Message::SpoolNote),
            checkbox(self.draft_archived)
                .label("Archived")
                .on_toggle(Message::SpoolArchived),
            text(format!(
                "remain {}% · {} g net / {} g initial · cloud {}",
                s.remain_percent,
                s.net_weight,
                s.initial_weight,
                if s.cloud_synced { "yes" } else { "local" }
            ))
            .size(12),
            row![
                text_input("use grams", &self.draft_use).on_input(Message::SpoolUseGrams),
                button("Use").on_press(Message::ApplySpoolUse),
            ]
            .spacing(6),
            row![
                text_input("measure gross g", &self.draft_measure)
                    .on_input(Message::SpoolMeasureGross),
                button("Measure").on_press(Message::ApplySpoolMeasure),
            ]
            .spacing(6),
        ]
        .spacing(6)
        .into()
    }
}
