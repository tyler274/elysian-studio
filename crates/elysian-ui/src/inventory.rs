//! Native Filament Manager: Spoolman-style inventory + SpoolmanDB catalog.

use iced::widget::{button, checkbox, column, row, scrollable, text, text_input};
use iced::{Element, Fill};

use elysian_protocol::{remain_percent, remaining_weight, InventorySpool};

use crate::theme;
use crate::Message;

fn quiet_btn<'a>(content: impl Into<Element<'a, Message>>) -> button::Button<'a, Message> {
    button(content)
        .padding([4, 10])
        .style(|_, status| theme::quiet(status))
}

fn field<'a>(
    placeholder: &'a str,
    value: &str,
    msg: fn(String) -> Message,
) -> text_input::TextInput<'a, Message> {
    text_input(placeholder, value)
        .on_input(msg)
        .style(theme::field)
}

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
            field(
                "search spools",
                &self.spool_search,
                Message::InventorySearch
            ),
            checkbox(self.show_archived)
                .label("Show archived")
                .on_toggle(Message::ShowArchived)
                .style(theme::tick),
            row![
                quiet_btn("New").on_press(Message::InventoryNew),
                quiet_btn("Save").on_press(Message::InventorySave),
                quiet_btn("Delete").on_press(Message::InventoryDelete),
            ]
            .spacing(6),
            row![
                quiet_btn("Pull").on_press(Message::InventoryPull),
                quiet_btn("Push").on_press(Message::InventoryPush),
                quiet_btn("Bind to slot").on_press(Message::BindSpoolToSlot),
            ]
            .spacing(6),
            text("Add from catalog").size(14),
            field(
                "vendor / material / color",
                &self.catalog_query,
                Message::CatalogQuery
            ),
        ];
        for sku in catalog_hits {
            let id = sku.external_id.clone();
            list = list.push(
                quiet_btn(
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
                quiet_btn(
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
            .style(theme::scroll)
            .into()
    }

    fn inventory_editor(&self) -> Element<'_, Message> {
        let s = &self.draft_spool;
        column![
            text("Edit spool").size(14),
            field("brand", &s.brand, Message::SpoolBrand),
            field("material", &s.material_type, Message::SpoolMaterial),
            field("series / name", &s.series, Message::SpoolSeries),
            field("color #RRGGBB", &s.color_code, Message::SpoolColor),
            field(
                "setting_id / filament_id",
                &s.filament_id,
                Message::SpoolFilamentId
            ),
            field(
                "location (e.g. A0S1)",
                &self.draft_location,
                Message::SpoolLocation
            ),
            field("note", &s.note, Message::SpoolNote),
            checkbox(self.draft_archived)
                .label("Archived")
                .on_toggle(Message::SpoolArchived)
                .style(theme::tick),
            text(format!(
                "remain {}% · {} g net / {} g initial · cloud {}",
                s.remain_percent,
                s.net_weight,
                s.initial_weight,
                if s.cloud_synced { "yes" } else { "local" }
            ))
            .size(12),
            row![
                field("use grams", &self.draft_use, Message::SpoolUseGrams),
                quiet_btn("Use").on_press(Message::ApplySpoolUse),
            ]
            .spacing(6),
            row![
                field(
                    "measure gross g",
                    &self.draft_measure,
                    Message::SpoolMeasureGross
                ),
                quiet_btn("Measure").on_press(Message::ApplySpoolMeasure),
            ]
            .spacing(6),
        ]
        .spacing(6)
        .into()
    }
}
