//! Studio notebook header: Home · Prepare / Preview / Device / Project /
//! Calibration / Filament Manager · Slice plate / Print plate.

use iced::widget::{
    button, column, container, image, progress_bar, row, shader, slider, stack, text,
    vertical_slider, Space,
};
use iced::{Alignment, ContentFit, Element, Fill, Padding};

use crate::theme;
use crate::{Message, ProcessTab, Workspace};
use elysian_gpu::CameraView;

impl crate::App {
    pub(crate) fn top_bar(&self) -> Element<'_, Message> {
        let tabs = row![
            self.file_header_group(),
            self.workspace_tab("⌂", Workspace::Home, theme::PREPARE),
            self.workspace_tab("Prepare", Workspace::Prepare, theme::PREPARE),
            self.workspace_tab("Preview", Workspace::Preview, theme::PREVIEW),
            self.workspace_tab("Device", Workspace::Device, theme::PREPARE),
            self.workspace_tab("Project", Workspace::Project, theme::PREPARE),
            self.workspace_tab("Calibration", Workspace::Calibration, theme::PREPARE),
            self.workspace_tab("Filament Manager", Workspace::Filament, theme::PREPARE),
        ]
        .spacing(4)
        .align_y(Alignment::Center);

        let actions = row![self.slice_header_group(), self.print_header_group()].spacing(6);

        row![tabs, Space::new().width(Fill), actions]
            .spacing(8)
            .padding(theme::HEADER_PAD)
            .align_y(Alignment::Center)
            .into()
    }

    fn file_header_group(&self) -> Element<'_, Message> {
        let open = self.file_menu_open;
        button(text("File").size(theme::BODY_SIZE).color(theme::TEXT))
            .padding([4, 10])
            .style(move |_, status| theme::menubar(open, status))
            .on_press(Message::ToggleFileMenu)
            .into()
    }

    pub(crate) fn header_menu_overlay(&self) -> Option<Element<'_, Message>> {
        let (menu, align_end) = if self.file_menu_open {
            (self.file_dropdown(), false)
        } else if self.slice_menu_open {
            (
                self.split_dropdown(&[
                    ("Slice plate", Message::SliceAll(false)),
                    ("Slice all", Message::SliceAll(true)),
                ]),
                true,
            )
        } else if self.print_menu_open {
            (
                self.split_dropdown(&[
                    ("Print plate", Message::PrintExport(false)),
                    ("Export plate sliced file", Message::PrintExport(true)),
                ]),
                true,
            )
        } else {
            return None;
        };
        let pad = if align_end {
            Padding {
                top: theme::HEADER_DROPDOWN_TOP,
                right: theme::HEADER_PAD[1] as f32,
                bottom: 0.0,
                left: 0.0,
            }
        } else {
            Padding {
                top: theme::HEADER_DROPDOWN_TOP,
                right: 0.0,
                bottom: 0.0,
                left: theme::HEADER_PAD[1] as f32,
            }
        };
        Some(if align_end {
            container(row![Space::new().width(Fill), menu])
                .width(Fill)
                .padding(pad)
                .into()
        } else {
            container(menu).padding(pad).into()
        })
    }

    fn file_dropdown(&self) -> Element<'_, Message> {
        let mut menu = column![].spacing(2);
        for (label, message) in [
            ("New Project", Message::NewProject),
            ("Open Project", Message::OpenModel),
            ("Save Project", Message::SaveProject),
            ("Save Project as", Message::SaveProjectAs),
            ("Import", Message::ImportModel),
        ] {
            menu = menu.push(dropdown_item(label, message));
        }
        if !self.recent_models.is_empty() {
            menu = menu.push(text("Recent").size(11).color(theme::TEXT_MUTED));
            for path in &self.recent_models {
                let label = path
                    .file_name()
                    .and_then(|n| n.to_str())
                    .unwrap_or("model")
                    .to_string();
                menu = menu.push(dropdown_item(label, Message::OpenRecent(path.clone())));
            }
        }
        container(menu)
            .padding(4)
            .width(220)
            .style(|_| theme::dropdown())
            .into()
    }

    fn split_dropdown(&self, options: &[(&'static str, Message)]) -> Element<'_, Message> {
        let mut menu = column![].spacing(2);
        for (label, message) in options {
            menu = menu.push(dropdown_item(*label, message.clone()));
        }
        container(menu)
            .padding(4)
            .width(220)
            .style(|_| theme::dropdown())
            .into()
    }

    fn slice_header_group(&self) -> Element<'_, Message> {
        let label = if self.slice_all {
            "Slice all"
        } else {
            "Slice plate"
        };
        let main = container(
            button(text(label).size(theme::BODY_SIZE).color(theme::PREPARE))
                .padding([4, 10])
                .style(|_, status| theme::slice_plate(status))
                .on_press(Message::Slice),
        )
        .style(|_| container::Style {
            border: iced::Border {
                color: theme::PREPARE,
                width: 1.0,
                radius: theme::RADIUS.into(),
            },
            ..container::Style::default()
        });
        self.header_split(Message::ToggleSliceMenu, main.into(), true)
    }

    fn print_header_group(&self) -> Element<'_, Message> {
        let label = if self.print_export {
            "Export plate sliced file"
        } else {
            "Print plate"
        };
        let main = container(
            button(text(label).size(theme::BODY_SIZE).color(iced::Color::WHITE))
                .padding([4, 10])
                .style(|_, status| theme::print_plate(status))
                .on_press(Message::Send),
        )
        .style(|_| container::Style {
            background: Some(iced::Background::Color(theme::PREPARE)),
            border: iced::Border {
                color: theme::PREPARE,
                width: 1.0,
                radius: theme::RADIUS.into(),
            },
            ..container::Style::default()
        });
        self.header_split(Message::TogglePrintMenu, main.into(), false)
    }

    fn header_split<'a>(
        &self,
        toggle: Message,
        main: Element<'a, Message>,
        outlined: bool,
    ) -> Element<'a, Message> {
        let chevron = button(text("▾").size(theme::BODY_SIZE))
            .padding([4, 6])
            .style(move |_, status| {
                if outlined {
                    theme::slice_plate(status)
                } else {
                    theme::print_plate(status)
                }
            })
            .on_press(toggle);
        row![chevron, main]
            .spacing(1)
            .align_y(Alignment::Center)
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

    pub(crate) fn home_page(&self) -> Element<'_, Message> {
        let mut recents =
            column![text("Recent").size(theme::TITLE_SIZE).color(theme::TEXT)].spacing(4);
        if self.recent_models.is_empty() {
            recents = recents.push(
                text("No recent files yet.")
                    .size(theme::BODY_SIZE)
                    .color(theme::TEXT_MUTED),
            );
        } else {
            for path in &self.recent_models {
                let label = path
                    .file_name()
                    .and_then(|n| n.to_str())
                    .unwrap_or("model")
                    .to_string();
                let body: Element<'_, Message> = if let Some(handle) = self.recent_thumb(path) {
                    row![
                        image(handle)
                            .width(72)
                            .height(72)
                            .content_fit(ContentFit::Contain),
                        text(label).size(theme::BODY_SIZE),
                    ]
                    .spacing(8)
                    .align_y(Alignment::Center)
                    .into()
                } else {
                    text(label).size(theme::BODY_SIZE).into()
                };
                recents = recents.push(
                    button(body)
                        .padding([4, 10])
                        .style(|_, status| theme::quiet(status))
                        .on_press(Message::OpenRecent(path.clone())),
                );
            }
        }
        container(
            column![
                text("Home").size(18).color(theme::TEXT),
                text("Open a project or recent model. MakerWorld is not embedded in this pass.")
                    .size(theme::BODY_SIZE)
                    .color(theme::TEXT_MUTED),
                button(
                    text("Open")
                        .size(theme::BODY_SIZE)
                        .color(iced::Color::WHITE)
                )
                .padding([4, 12])
                .style(|_, status| theme::print_plate(status))
                .on_press(Message::OpenModel),
                recents,
            ]
            .spacing(12)
            .padding(24),
        )
        .width(Fill)
        .height(Fill)
        .style(|_| theme::sidebar_pane())
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
        if self.load_progress.is_some() {
            stack![viewport, overlays, self.load_progress_overlay()]
                .width(Fill)
                .height(Fill)
                .into()
        } else {
            stack![viewport, overlays].width(Fill).height(Fill).into()
        }
    }

    fn load_progress_overlay(&self) -> Element<'_, Message> {
        let (label, fraction) = self
            .load_progress
            .as_ref()
            .map(|p| (p.label.clone(), p.fraction))
            .unwrap_or_else(|| (String::new(), 0.0));
        let card = container(
            column![
                text("Opening project")
                    .size(theme::TITLE_SIZE)
                    .color(theme::TEXT),
                text(label).size(theme::BODY_SIZE).color(theme::TEXT_MUTED),
                progress_bar(0.0..=1.0, fraction).girth(8),
            ]
            .spacing(10),
        )
        .padding(18)
        .width(320)
        .style(|_| theme::card());
        container(card)
            .width(Fill)
            .height(Fill)
            .align_x(iced::alignment::Horizontal::Center)
            .align_y(iced::alignment::Vertical::Center)
            .into()
    }

    fn viewport_overlays(&self) -> Element<'_, Message> {
        let plate_name = text(self.plate_label())
            .size(theme::BODY_SIZE)
            .color(theme::PLATE_ORANGE);
        let badge = text(format!("{:02}", self.plate + 1))
            .size(18)
            .color(theme::TEXT);
        let mut meta = column![row![plate_name, badge].spacing(8)];
        for toast in self.toasts.iter().rev().take(3) {
            meta = meta.push(text(toast).size(12).color(theme::TEXT));
        }
        meta = meta.push(text(self.plate_hint()).size(11).color(theme::TEXT_MUTED));

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

        let mut bottom = column![self.view_cube(), meta].spacing(8);
        if self.workspace == Workspace::Preview {
            let max_move = self.scene.toolpaths.vertices.len().saturating_sub(1) as f64;
            bottom = bottom.push(
                slider(
                    0.0..=max_move.max(1.0),
                    f64::from(self.scene.preview_vertices),
                    |v| Message::PreviewMove(v as u32),
                )
                .step(1.0)
                .style(theme::range),
            );
        }

        let chrome: Element<'_, Message> =
            column![top, Space::new().width(Fill).height(Fill), bottom]
                .padding(8)
                .width(Fill)
                .height(Fill)
                .into();

        let mut left = row![self.plate_thumb_strip()].spacing(4);
        if self.workspace == Workspace::Prepare {
            left = left.push(self.plater_toolbar());
        }
        if self.workspace == Workspace::Preview {
            let max_layer = self.scene.toolpaths.layer_zs.len().saturating_sub(1) as f64;
            let layer = vertical_slider(
                0.0..=max_layer.max(1.0),
                f64::from(self.scene.preview_layer),
                |v| Message::PreviewLayer(v as u32),
            )
            .step(1.0)
            .style(theme::range);
            left = left.push(
                container(layer)
                    .padding(iced::Padding {
                        top: 48.0,
                        right: 4.0,
                        bottom: 72.0,
                        left: 4.0,
                    })
                    .height(Fill)
                    .width(28),
            );
        }

        row![left, chrome, self.collapse_chevron()]
            .width(Fill)
            .height(Fill)
            .into()
    }

    fn plate_thumb_strip(&self) -> Element<'_, Message> {
        let n = self.plate_count();
        let mut col = column![].spacing(4);
        for i in 0..n.max(1) {
            let active = i == self.plate;
            let color = if active {
                iced::Color::WHITE
            } else {
                theme::TEXT_MUTED
            };
            col = col.push(
                container(
                    button(text(format!("{}", i + 1)).size(12).color(color))
                        .padding([6, 10])
                        .style(move |_, status| theme::process_tab(active, status))
                        .on_press(Message::Plate(i)),
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
                        theme::chip()
                    }
                }),
            );
        }
        col.padding([8, 4]).into()
    }

    fn view_cube(&self) -> Element<'_, Message> {
        let face = |label: &'static str, view: CameraView| {
            button(text(label).size(11))
                .padding([2, 6])
                .style(|_, status| theme::quiet(status))
                .on_press(Message::CameraView(view))
        };
        column![
            row![
                face("Iso", CameraView::Iso),
                face("Top", CameraView::Top),
                face("Fit", CameraView::Fit),
            ]
            .spacing(2),
            row![
                face("Front", CameraView::Front),
                face("Back", CameraView::Back),
            ]
            .spacing(2),
            row![
                face("Left", CameraView::Left),
                face("Right", CameraView::Right),
            ]
            .spacing(2),
        ]
        .spacing(2)
        .into()
    }

    fn collapse_chevron(&self) -> Element<'_, Message> {
        let label = if self.sidebar_collapsed { "‹" } else { "›" };
        container(
            button(text(label).size(16))
                .padding([8, 6])
                .style(|_, status| theme::quiet(status))
                .on_press(Message::CollapseSidebar),
        )
        .height(Fill)
        .align_y(Alignment::Center)
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

fn dropdown_item<'a>(label: impl Into<String>, message: Message) -> Element<'a, Message> {
    button(text(label.into()).size(12))
        .padding([6, 12])
        .width(Fill)
        .style(|_, status| theme::dropdown_item(status))
        .on_press(message)
        .into()
}
