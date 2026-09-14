//! Studio notebook header: Home · Prepare / Preview / Device / Project /
//! Calibration / Filament Manager · Slice plate / Print plate.

use iced::widget::{button, column, container, row, shader, stack, text, Space};
use iced::{Alignment, Element, Fill};

use crate::theme;
use crate::{Message, ProcessTab, Workspace};

impl crate::App {
    pub(crate) fn top_bar(&self) -> Element<'_, Message> {
        let tabs = row![
            self.quiet_tab("⌂", Message::Workspace(Workspace::Prepare)),
            self.workspace_tab("Prepare", Workspace::Prepare, theme::PREPARE),
            self.workspace_tab("Preview", Workspace::Preview, theme::PREVIEW),
            self.workspace_tab("Device", Workspace::Device, theme::PREPARE),
            self.workspace_tab("Project", Workspace::Project, theme::PREPARE),
            self.workspace_tab("Calibration", Workspace::Calibration, theme::PREPARE),
            self.workspace_tab("Filament Manager", Workspace::Filament, theme::PREPARE),
        ]
        .spacing(4)
        .align_y(Alignment::Center);

        let actions = row![
            container(
                button(
                    text("Slice plate")
                        .size(theme::BODY_SIZE)
                        .color(theme::PREPARE)
                )
                .padding([4, 10])
                .style(|_, status| theme::slice_plate(status))
                .on_press(Message::Slice)
            )
            .style(|_| container::Style {
                border: iced::Border {
                    color: theme::PREPARE,
                    width: 1.0,
                    radius: theme::RADIUS.into(),
                },
                ..container::Style::default()
            }),
            container(
                button(
                    text("Print plate")
                        .size(theme::BODY_SIZE)
                        .color(iced::Color::WHITE)
                )
                .padding([4, 10])
                .style(|_, status| theme::print_plate(status))
                .on_press(Message::Send)
            )
            .style(|_| container::Style {
                background: Some(iced::Background::Color(theme::PREPARE)),
                border: iced::Border {
                    color: theme::PREPARE,
                    width: 1.0,
                    radius: theme::RADIUS.into(),
                },
                ..container::Style::default()
            }),
        ]
        .spacing(6);

        row![tabs, Space::new().width(Fill), actions]
            .spacing(8)
            .padding(theme::HEADER_PAD)
            .align_y(Alignment::Center)
            .into()
    }

    fn quiet_tab(&self, label: &'static str, message: Message) -> Element<'_, Message> {
        button(text(label).size(theme::BODY_SIZE))
            .padding([4, 10])
            .style(|_, status| theme::quiet(status))
            .on_press(message)
            .into()
    }

    fn workspace_tab(
        &self,
        label: &'static str,
        workspace: Workspace,
        fill: iced::Color,
    ) -> Element<'_, Message> {
        let active = self.workspace == workspace;
        let label_color = if active {
            iced::Color::WHITE
        } else {
            theme::TEXT_MUTED
        };
        container(
            button(text(label).size(theme::BODY_SIZE).color(label_color))
                .padding([4, 12])
                .style(move |_, status| theme::notebook_tab(active, fill, status))
                .on_press(Message::Workspace(workspace)),
        )
        .style(move |_| {
            if active {
                container::Style {
                    background: Some(iced::Background::Color(fill)),
                    border: iced::Border {
                        color: fill,
                        width: 1.0,
                        radius: theme::RADIUS.into(),
                    },
                    ..container::Style::default()
                }
            } else {
                container::Style::default()
            }
        })
        .into()
    }

    pub(crate) fn stub_page(
        &self,
        title: &'static str,
        body: &'static str,
    ) -> Element<'_, Message> {
        container(
            column![
                text(title).size(18).color(theme::TEXT),
                text(body).size(theme::BODY_SIZE).color(theme::TEXT_MUTED),
            ]
            .spacing(8)
            .padding(24),
        )
        .width(Fill)
        .height(Fill)
        .style(|_| theme::sidebar_pane())
        .into()
    }

    pub(crate) fn viewport_stage(&self) -> Element<'_, Message> {
        let viewport = shader(&self.scene).width(Fill).height(Fill);
        let overlays = self.viewport_overlays();
        let stacked: Element<'_, Message> =
            stack![viewport, overlays].width(Fill).height(Fill).into();
        if self.workspace == Workspace::Prepare {
            column![
                container(self.plater_toolbar())
                    .width(Fill)
                    .style(|_| theme::header_bar()),
                stacked
            ]
            .height(Fill)
            .into()
        } else {
            stacked
        }
    }

    fn viewport_overlays(&self) -> Element<'_, Message> {
        let plate_name = text(self.plate_label())
            .size(theme::BODY_SIZE)
            .color(theme::PLATE_ORANGE);
        let badge = text(format!("{:02}", self.plate + 1))
            .size(18)
            .color(theme::TEXT);
        let mut bottom = column![row![plate_name, badge].spacing(8)];
        if self.show_left_nozzle_only() {
            bottom = bottom.push(
                text("Left nozzle only area")
                    .size(12)
                    .color(theme::TEXT_MUTED),
            );
        }
        for toast in self.toasts.iter().rev().take(3) {
            bottom = bottom.push(text(toast).size(12).color(theme::TEXT));
        }
        bottom = bottom.push(text(self.plate_hint()).size(11).color(theme::TEXT_MUTED));

        let mut top = row![].spacing(8).align_y(Alignment::Center);
        if self.workspace == Workspace::Preview {
            top = top.push(self.plate_switcher());
            top = top.push(Space::new().width(Fill));
            top = top.push(
                text(format!("{:.0} FPS", self.fps))
                    .size(12)
                    .color(theme::TEXT),
            );
        } else {
            top = top.push(Space::new().width(Fill));
        }

        column![top, Space::new().width(Fill).height(Fill), bottom]
            .padding(8)
            .width(Fill)
            .height(Fill)
            .into()
    }

    fn plate_switcher(&self) -> Element<'_, Message> {
        row![
            button(text("<").size(12))
                .padding([2, 8])
                .style(|_, status| theme::quiet(status))
                .on_press(Message::PlatePrev),
            container(text(format!("{}", self.plate + 1)).size(12))
                .padding([4, 10])
                .style(|_| theme::chip()),
            button(text(">").size(12))
                .padding([2, 8])
                .style(|_, status| theme::quiet(status))
                .on_press(Message::PlateNext),
        ]
        .spacing(4)
        .align_y(Alignment::Center)
        .into()
    }

    pub(crate) fn process_tabs(&self) -> Element<'_, Message> {
        let tab = |label: &'static str, tab: ProcessTab| {
            let active = self.quality_tab == tab;
            let color = if active {
                iced::Color::WHITE
            } else {
                theme::TEXT_MUTED
            };
            container(
                button(text(label).size(12).color(color))
                    .padding([3, 8])
                    .style(move |_, status| theme::process_tab(active, status))
                    .on_press(Message::ProcessTab(tab)),
            )
            .style(move |_| {
                if active {
                    container::Style {
                        background: Some(iced::Background::Color(theme::PREPARE)),
                        border: iced::Border {
                            radius: theme::RADIUS.into(),
                            width: 0.0,
                            color: iced::Color::TRANSPARENT,
                        },
                        ..container::Style::default()
                    }
                } else {
                    container::Style::default()
                }
            })
        };
        row![
            tab("Quality", ProcessTab::Quality),
            tab("Strength", ProcessTab::Strength),
            tab("Speed", ProcessTab::Speed),
            tab("Support", ProcessTab::Support),
            tab("Others", ProcessTab::Others),
        ]
        .spacing(4)
        .into()
    }
}
