//! Studio notebook header: Prepare / Preview / Device / Filament plus Open / Slice / Send.

use iced::widget::{button, row, text};
use iced::{Element, Fill};

use crate::{Message, Workspace};

impl crate::App {
    pub(crate) fn top_bar(&self) -> Element<'_, Message> {
        row![
            self.workspace_tab("Prepare", Workspace::Prepare),
            self.workspace_tab("Preview", Workspace::Preview),
            self.workspace_tab("Device", Workspace::Device),
            self.workspace_tab("Filament", Workspace::Filament),
            button("Open").padding([3, 10]).on_press(Message::OpenModel),
            button("Slice").padding([3, 10]).on_press(Message::Slice),
            button("Send last slice")
                .padding([3, 10])
                .on_press(Message::Send),
            text(&self.status).size(13).width(Fill),
        ]
        .spacing(6)
        .padding([4, 10])
        .into()
    }

    fn workspace_tab(&self, label: &'static str, workspace: Workspace) -> Element<'_, Message> {
        let caption = if self.workspace == workspace {
            format!("[{label}]")
        } else {
            label.to_string()
        };
        button(text(caption).size(13))
            .padding([3, 10])
            .on_press(Message::Workspace(workspace))
            .into()
    }
}
