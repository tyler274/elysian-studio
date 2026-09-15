//! Prepare / Preview right panes. `App` / `Message` stay in the crate root.

use elysian_config::{SeamPosition, TopOneWallType};
use iced::widget::{
    button, checkbox, column, container, keyed_column, pick_list, row, scrollable, slider, text,
    text_input,
};
use iced::{Alignment, Element, Fill, Length};

use crate::theme;
use crate::{format_eta, role_legend, Message, ProcessTab, SIDEBAR_WIDTH};

const SEAM_LABELS: [&str; 4] = ["aligned", "rear", "nearest", "random"];

impl crate::App {
    pub(crate) fn prepare_sidebar(&self) -> Element<'_, Message> {
        scrollable(
            keyed_column([
                (
                    0u8,
                    self.section(
                        "Printer",
                        self.printer_open,
                        Message::TogglePrinterSection,
                        self.printer_body(true),
                    ),
                ),
                (
                    1u8,
                    self.section(
                        "Filament",
                        self.filament_open,
                        Message::ToggleFilamentSection,
                        self.filament_library(),
                    ),
                ),
                (
                    2u8,
                    self.section(
                        "Process",
                        self.process_open,
                        Message::ToggleProcessSection,
                        self.process_pane(true),
                    ),
                ),
            ])
            .spacing(8)
            .padding(theme::SIDEBAR_PAD)
            .width(SIDEBAR_WIDTH),
        )
        .style(theme::scroll)
        .height(Fill)
        .into()
    }

    pub(crate) fn preview_sidebar(&self) -> Element<'_, Message> {
        let max_layer = self.scene.toolpaths.layer_zs.len().saturating_sub(1) as f64;
        let max_move = self.scene.toolpaths.vertices.len().saturating_sub(1) as f64;
        let preview_z = self.scene.preview_z();
        let preview_z = if preview_z.is_finite() && preview_z.abs() < 1.0e5 {
            preview_z
        } else {
            0.0
        };
        let layer_frac = if max_layer <= 0.0 {
            1.0
        } else {
            f64::from(self.scene.preview_layer) / max_layer
        };
        let eta = self
            .estimated_seconds
            .map(|s| format_eta(s * layer_frac))
            .unwrap_or_else(|| "—".into());
        scrollable(
            column![
                keyed_column([
                    (
                        0u8,
                        self.section(
                            "Printer",
                            self.printer_open,
                            Message::TogglePrinterSection,
                            self.printer_body(false),
                        ),
                    ),
                    (
                        1u8,
                        self.section(
                            "Filament",
                            self.filament_open,
                            Message::ToggleFilamentSection,
                            self.preview_filament_chips(),
                        ),
                    ),
                    (
                        2u8,
                        self.section(
                            "Process",
                            self.process_open,
                            Message::ToggleProcessSection,
                            self.process_pane(false),
                        ),
                    ),
                ])
                .spacing(8),
                text(format!(
                    "Layer {}  Z {:.2} mm  ~{eta}",
                    self.scene.preview_layer + 1,
                    preview_z
                ))
                .size(12)
                .color(theme::TEXT_MUTED),
                slider(
                    0.0..=max_layer.max(1.0),
                    f64::from(self.scene.preview_layer),
                    |v| Message::PreviewLayer(v as u32)
                )
                .step(1.0)
                .style(theme::range),
                slider(
                    0.0..=max_move.max(1.0),
                    f64::from(self.scene.preview_vertices),
                    |v| Message::PreviewMove(v as u32)
                )
                .step(1.0)
                .style(theme::range),
                text(role_legend()).size(11).color(theme::TEXT_MUTED),
                checkbox(self.scene.hide_infill)
                    .label("Hide infill")
                    .on_toggle(Message::HideInfill)
                    .style(theme::tick),
                checkbox(self.scene.hide_support)
                    .label("Hide support")
                    .on_toggle(Message::HideSupport)
                    .style(theme::tick),
                checkbox(self.scene.realistic)
                    .label("Realistic")
                    .on_toggle(Message::Realistic)
                    .style(theme::tick),
            ]
            .spacing(8)
            .padding(theme::SIDEBAR_PAD)
            .width(SIDEBAR_WIDTH),
        )
        .style(theme::scroll)
        .height(Fill)
        .into()
    }

    fn section<'a>(
        &self,
        title: &'static str,
        open: bool,
        toggle: Message,
        body: Element<'a, Message>,
    ) -> Element<'a, Message> {
        let chevron = if open { "▾" } else { "▸" };
        let header = button(
            row![
                text(chevron)
                    .size(theme::BODY_SIZE)
                    .color(theme::TEXT_MUTED),
                text(title).size(theme::TITLE_SIZE).color(theme::TEXT),
            ]
            .spacing(6)
            .align_y(Alignment::Center),
        )
        .padding([4, 8])
        .width(Fill)
        .style(|_, status| theme::quiet(status))
        .on_press(toggle);
        let body = container(body).width(Fill).clip(true);
        let body = if open {
            body
        } else {
            body.height(Length::Fixed(0.0))
        };
        container(column![header, body].spacing(6))
            .padding(8)
            .width(Fill)
            .style(|_| theme::card())
            .into()
    }

    fn printer_body(&self, with_ams: bool) -> Element<'_, Message> {
        let machine_names: Vec<String> = self
            .machine_profiles
            .iter()
            .map(|p| p.name.clone())
            .collect();
        let mut col = column![
            pick_list(
                machine_names,
                self.machine_name.clone(),
                Message::MachineProfile
            )
            .placeholder("machine JSON")
            .padding([3, 8])
            .text_size(theme::BODY_SIZE)
            .style(theme::choice)
            .menu_style(theme::menu),
            text(format!(
                "Bed {:.0}×{:.0} mm · {}",
                self.scene.bed.width(),
                self.scene.bed.height(),
                self.settings.curr_bed_type
            ))
            .size(12)
            .color(theme::TEXT_MUTED),
            row![
                button(text("Bed texture").size(12))
                    .padding([3, 8])
                    .style(|_, status| theme::quiet(status))
                    .on_press(Message::Toast(
                        "Bed texture uses the printer profile".into()
                    )),
                button(text("Sync").size(12))
                    .padding([3, 8])
                    .style(|_, status| theme::quiet(status))
                    .on_press(Message::Toast("Printer sync is a placeholder".into())),
            ]
            .spacing(6),
            self.plate_buttons(),
        ]
        .spacing(6);
        if with_ams {
            col = col.push(self.ams_cards());
        }
        col.into()
    }

    fn ams_cards(&self) -> Element<'_, Message> {
        row![self.ams_card(1), self.ams_card(2)].spacing(8).into()
    }

    fn ams_card(&self, n: u8) -> Element<'_, Message> {
        let ams_id = n.saturating_sub(1);
        let trays: Vec<_> = self
            .ams
            .trays
            .iter()
            .filter(|t| t.ams_id == ams_id)
            .collect();
        let body: Element<'_, Message> = if trays.is_empty() {
            text("Not installed")
                .size(theme::BODY_SIZE)
                .color(theme::TEXT_MUTED)
                .into()
        } else {
            let mut slots = row![].spacing(4);
            for tray in trays {
                let color = crate::monitor::parse_tray_color(&tray.color);
                let kind = if tray.filament_type.is_empty() {
                    "—"
                } else {
                    tray.filament_type.as_str()
                };
                slots = slots.push(
                    column![
                        container(text(" ").size(6))
                            .width(18)
                            .height(10)
                            .style(move |_| container::Style {
                                background: Some(iced::Background::Color(color)),
                                border: iced::Border {
                                    color: theme::CARD_BORDER,
                                    width: 1.0,
                                    radius: 2.0.into(),
                                },
                                ..container::Style::default()
                            }),
                        text(kind).size(10).color(theme::TEXT_MUTED),
                    ]
                    .spacing(2),
                );
            }
            slots.into()
        };
        container(
            column![
                text(format!("AMS-{n}")).size(12).color(theme::TEXT),
                body,
                text(format!(
                    "Ø {:.2} mm · flow {:.2}",
                    self.settings.filament_diameter_mm, self.settings.flow_ratio
                ))
                .size(11)
                .color(theme::TEXT_MUTED),
            ]
            .spacing(4),
        )
        .padding(10)
        .width(Fill)
        .style(|_| theme::card())
        .into()
    }

    fn preview_filament_chips(&self) -> Element<'_, Message> {
        let mut chips = row![].spacing(6);
        for i in 0..4 {
            let (label, colour) = self
                .filament_slots
                .get(i)
                .map(|s| (s.name.as_str(), s.colour.as_str()))
                .unwrap_or(("—", "#2A2A2AFF"));
            let parsed = crate::slot_colour_hex(colour);
            let r = u8::from_str_radix(parsed.get(1..3).unwrap_or("40"), 16).unwrap_or(40);
            let g = u8::from_str_radix(parsed.get(3..5).unwrap_or("40"), 16).unwrap_or(40);
            let b = u8::from_str_radix(parsed.get(5..7).unwrap_or("40"), 16).unwrap_or(40);
            chips = chips.push(
                container(
                    column![
                        container(text(" ").size(8))
                            .width(28)
                            .height(18)
                            .style(move |_| container::Style {
                                background: Some(iced::Background::Color(iced::Color::from_rgb8(
                                    r, g, b,
                                ))),
                                border: iced::Border {
                                    color: theme::CARD_BORDER,
                                    width: 1.0,
                                    radius: 3.0.into(),
                                },
                                ..container::Style::default()
                            }),
                        text(format!("{}", i + 1)).size(10).color(theme::TEXT_MUTED),
                    ]
                    .spacing(2),
                )
                .padding(4)
                .style(|_| theme::chip()),
            );
            let _ = label;
        }
        chips.into()
    }

    fn process_pane(&self, full_notebook: bool) -> Element<'_, Message> {
        let process_names: Vec<String> = self
            .process_profiles
            .iter()
            .filter(|p| {
                self.process_search.is_empty()
                    || p.name
                        .to_ascii_lowercase()
                        .contains(&self.process_search.to_ascii_lowercase())
            })
            .map(|p| p.name.clone())
            .collect();
        let mut col = column![
            row![
                button(text("Global").size(12))
                    .padding([3, 8])
                    .style(move |_, status| theme::notebook_tab(
                        !self.process_objects,
                        theme::PREPARE,
                        status
                    ))
                    .on_press(Message::ProcessObjects(false)),
                button(text("Objects").size(12))
                    .padding([3, 8])
                    .style(move |_, status| theme::notebook_tab(
                        self.process_objects,
                        theme::PREPARE,
                        status
                    ))
                    .on_press(Message::ProcessObjects(true)),
            ]
            .spacing(4),
            text_input("search process", &self.process_search)
                .on_input(Message::ProcessSearch)
                .style(theme::field),
            pick_list(
                process_names,
                self.process_name.clone(),
                Message::ProcessProfile
            )
            .placeholder("process JSON")
            .padding([3, 8])
            .text_size(theme::BODY_SIZE)
            .style(theme::choice)
            .menu_style(theme::menu),
        ]
        .spacing(6);
        if full_notebook {
            if self.process_objects {
                col = col.push(self.object_panel());
                col = col.push(self.transform_panel());
                col = col.push(self.paint_panel());
            } else {
                col = col.push(self.process_tabs());
                col = col.push(self.process_page());
            }
        } else {
            col = col.push(self.preview_quality());
        }
        col.into()
    }

    fn paint_panel(&self) -> Element<'_, Message> {
        column![
            text("Paint").size(theme::TITLE_SIZE).color(theme::TEXT),
            checkbox(self.paint_blocker)
                .label("Blocker (else Enforcer)")
                .on_toggle(Message::PaintBlocker)
                .style(theme::tick),
            text(format!("Brush {:.1} mm", self.brush_mm))
                .size(12)
                .color(theme::TEXT_MUTED),
            slider(0.5..=12.0, f64::from(self.brush_mm), |v| {
                Message::BrushRadius(v as f32)
            })
            .step(0.5)
            .style(theme::range),
            row![
                button(text("Support").size(12))
                    .padding([3, 8])
                    .style(|_, status| theme::quiet(status))
                    .on_press(Message::PaintSupport),
                button(text("Seam").size(12))
                    .padding([3, 8])
                    .style(|_, status| theme::quiet(status))
                    .on_press(Message::PaintSeam),
                button(text("Fuzzy").size(12))
                    .padding([3, 8])
                    .style(|_, status| theme::quiet(status))
                    .on_press(Message::PaintFuzzy),
                button(text("Off").size(12))
                    .padding([3, 8])
                    .style(|_, status| theme::quiet(status))
                    .on_press(Message::PaintClear),
            ]
            .spacing(4),
        ]
        .spacing(4)
        .into()
    }

    fn process_page(&self) -> Element<'_, Message> {
        match self.quality_tab {
            ProcessTab::Quality => self.quality_page(),
            ProcessTab::Strength => column![
                text(format!("Walls {}", self.settings.wall_loops)).size(theme::BODY_SIZE),
                row![
                    button("-")
                        .padding([3, 8])
                        .style(|_, status| theme::quiet(status))
                        .on_press(Message::WallLoops(
                            self.settings.wall_loops.saturating_sub(1).max(1)
                        )),
                    button("+")
                        .padding([3, 8])
                        .style(|_, status| theme::quiet(status))
                        .on_press(Message::WallLoops((self.settings.wall_loops + 1).min(10))),
                ]
                .spacing(6),
                text(format!(
                    "Sparse infill {:.0}%",
                    self.settings.infill_density * 100.0
                ))
                .size(theme::BODY_SIZE),
                slider(0.0..=1.0, self.settings.infill_density, Message::Infill)
                    .step(0.01)
                    .style(theme::range),
            ]
            .spacing(6)
            .into(),
            ProcessTab::Speed => column![
                text(format!("Layer {:.2} mm", self.settings.layer_height_mm))
                    .size(theme::BODY_SIZE),
                slider(
                    0.08..=0.32,
                    self.settings.layer_height_mm,
                    Message::LayerHeight
                )
                .step(0.01)
                .style(theme::range),
                text(format!("Nozzle {}°C", self.settings.temperature_c)).size(theme::BODY_SIZE),
                slider(
                    180.0..=280.0,
                    f64::from(self.settings.temperature_c),
                    Message::NozzleTemp,
                )
                .step(1.0)
                .style(theme::range),
            ]
            .spacing(6)
            .into(),
            ProcessTab::Support => column![
                checkbox(self.settings.enable_support)
                    .label("Supports")
                    .on_toggle(Message::EnableSupport)
                    .style(theme::tick),
                checkbox(self.by_object)
                    .label("By-object sequence")
                    .on_toggle(Message::ByObject)
                    .style(theme::tick),
            ]
            .spacing(6)
            .into(),
            ProcessTab::Others => column![
                checkbox(self.scene.realistic)
                    .label("Realistic preview")
                    .on_toggle(Message::Realistic)
                    .style(theme::tick),
                button(text("Reset camera").size(12))
                    .padding([3, 8])
                    .style(|_, status| theme::quiet(status))
                    .on_press(Message::ResetCamera),
            ]
            .spacing(6)
            .into(),
        }
    }

    fn quality_page(&self) -> Element<'_, Message> {
        column![
            text(format!(
                "Sparse infill {:.0}% {}",
                self.settings.infill_density * 100.0,
                self.settings.infill_pattern.as_str()
            ))
            .size(theme::BODY_SIZE)
            .color(theme::TEXT),
            slider(0.0..=1.0, self.settings.infill_density, Message::Infill)
                .step(0.01)
                .style(theme::range),
            text(format!(
                "Line width {:.2} · outer {:.2} · inner {:.2} · sparse {:.2}",
                self.settings.line_width_mm,
                self.settings.outer_wall_line_width_mm,
                self.settings.inner_wall_line_width_mm,
                self.settings.sparse_infill_line_width_mm
            ))
            .size(12)
            .color(theme::TEXT_MUTED),
            self.seam_group(),
        ]
        .spacing(6)
        .into()
    }

    fn preview_quality(&self) -> Element<'_, Message> {
        column![
            text("Quality").size(theme::TITLE_SIZE).color(theme::TEXT),
            text(format!("Layer {:.2} mm", self.settings.layer_height_mm)).size(theme::BODY_SIZE),
            slider(
                0.08..=0.32,
                self.settings.layer_height_mm,
                Message::LayerHeight
            )
            .step(0.01)
            .style(theme::range),
            text(format!(
                "First layer {:.2} mm",
                self.settings.first_layer_height_mm
            ))
            .size(theme::BODY_SIZE),
            slider(
                0.08..=0.4,
                self.settings.first_layer_height_mm,
                Message::FirstLayerHeight
            )
            .step(0.01)
            .style(theme::range),
            self.seam_pick(),
            checkbox(self.settings.precise_outer_wall)
                .label("Precise wall")
                .on_toggle(Message::PreciseOuterWall)
                .style(theme::tick),
            checkbox(self.settings.only_one_wall_first_layer)
                .label("Only one wall first layer")
                .on_toggle(Message::OnlyOneWallFirst)
                .style(theme::tick),
            checkbox(self.settings.top_one_wall == TopOneWallType::AllTop)
                .label("Only one wall top")
                .on_toggle(Message::OnlyOneWallTop)
                .style(theme::tick),
        ]
        .spacing(6)
        .into()
    }

    fn seam_group(&self) -> Element<'_, Message> {
        column![
            text("Seam").size(theme::TITLE_SIZE).color(theme::TEXT),
            self.seam_pick(),
            checkbox(self.settings.seam_placement_away_from_overhangs)
                .label("Staggered inner seams")
                .on_toggle(Message::SeamAway)
                .style(theme::tick),
            checkbox(self.settings.seam_slope_conditional)
                .label("Apply scarf around sharp corners")
                .on_toggle(Message::ScarfConditional)
                .style(theme::tick),
            text(format!(
                "Scarf angle ≥ {}°",
                self.settings.scarf_angle_threshold_deg
            ))
            .size(12)
            .color(theme::TEXT_MUTED),
            checkbox(self.settings.seam_slope_entire_loop)
                .label("Scarf around entire wall")
                .on_toggle(Message::ScarfEntireLoop)
                .style(theme::tick),
            text(format!("Scarf steps {}", self.settings.seam_slope_steps))
                .size(12)
                .color(theme::TEXT_MUTED),
            checkbox(self.settings.seam_slope_inner_walls)
                .label("Scarf on inner walls")
                .on_toggle(Message::ScarfInnerWalls)
                .style(theme::tick),
            checkbox(self.settings.override_filament_scarf_seam_setting)
                .label("Override filament scarf settings")
                .on_toggle(Message::OverrideScarf)
                .style(theme::tick),
        ]
        .spacing(4)
        .into()
    }

    fn seam_pick(&self) -> Element<'_, Message> {
        column![
            text("Seam position").size(12).color(theme::TEXT_MUTED),
            pick_list(
                SEAM_LABELS
                    .iter()
                    .map(|s| (*s).to_string())
                    .collect::<Vec<_>>(),
                Some(self.settings.seam.as_str().to_string()),
                |label| {
                    Message::Seam(SeamPosition::from_name(&label).unwrap_or(SeamPosition::Aligned))
                }
            )
            .padding([3, 8])
            .text_size(theme::BODY_SIZE)
            .style(theme::choice)
            .menu_style(theme::menu),
        ]
        .spacing(4)
        .into()
    }
}
