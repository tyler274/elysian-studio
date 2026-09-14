//! Studio/Orca chrome tokens. Widget `style` closures use these instead of
//! iced's stock `Theme::Dark` greens/purples.

use iced::widget::{button, container};
use iced::{Background, Border, Color, Theme};
use iced::theme::Palette;

/// Header bar ~`rgb(0.07, 0.08, 0.10)`.
pub const HEADER: Color = Color::from_rgb(0.07, 0.08, 0.10);
/// Sidebar ~`rgb(0.10, 0.11, 0.13)`.
pub const SIDEBAR: Color = Color::from_rgb(0.10, 0.11, 0.13);
/// Nested printer / AMS / process cards.
pub const CARD: Color = Color::from_rgb(0.13, 0.14, 0.16);
pub const CARD_BORDER: Color = Color::from_rgb(0.18, 0.20, 0.22);
pub const INACTIVE: Color = Color::from_rgb(0.16, 0.17, 0.19);
pub const INACTIVE_HOVER: Color = Color::from_rgb(0.20, 0.22, 0.24);
pub const TEXT: Color = Color::from_rgb(0.90, 0.90, 0.90);
pub const TEXT_MUTED: Color = Color::from_rgb(0.62, 0.64, 0.66);
/// Active Prepare fill: Studio green `#00AE42`.
pub const PREPARE: Color = Color::from_rgb8(0x00, 0xAE, 0x42);
/// Active Preview fill: Orca teal `#1A9B8A`.
pub const PREVIEW: Color = Color::from_rgb8(0x1A, 0x9B, 0x8A);
/// Plate name overlay.
pub const PLATE_ORANGE: Color = Color::from_rgb8(0xFF, 0x8A, 0x1A);
pub const RADIUS: f32 = 4.0;

pub const HEADER_PAD: [u16; 2] = [4, 10];
pub const SIDEBAR_PAD: [u16; 2] = [10, 12];
pub const BODY_SIZE: u32 = 13;
pub const TITLE_SIZE: u32 = 14;

pub fn studio() -> Theme {
    Theme::custom(
        "Studio",
        Palette {
            background: HEADER,
            text: TEXT,
            primary: PREPARE,
            success: PREPARE,
            warning: PLATE_ORANGE,
            danger: Color::from_rgb8(0xC3, 0x42, 0x3F),
        },
    )
}

pub fn header_bar() -> container::Style {
    container::Style {
        background: Some(Background::Color(HEADER)),
        text_color: Some(TEXT),
        ..container::Style::default()
    }
}

pub fn sidebar_pane() -> container::Style {
    container::Style {
        background: Some(Background::Color(SIDEBAR)),
        text_color: Some(TEXT),
        ..container::Style::default()
    }
}

pub fn card() -> container::Style {
    container::Style {
        background: Some(Background::Color(CARD)),
        text_color: Some(TEXT),
        border: Border {
            color: CARD_BORDER,
            width: 1.0,
            radius: 6.0.into(),
        },
        ..container::Style::default()
    }
}

pub fn chip() -> container::Style {
    container::Style {
        background: Some(Background::Color(INACTIVE)),
        border: Border {
            color: CARD_BORDER,
            width: 1.0,
            radius: RADIUS.into(),
        },
        ..container::Style::default()
    }
}

pub fn active_tab_fill(workspace: crate::Workspace) -> Color {
    match workspace {
        crate::Workspace::Preview => PREVIEW,
        _ => PREPARE,
    }
}

pub fn active_tab_hex(workspace: crate::Workspace) -> &'static str {
    match workspace {
        crate::Workspace::Preview => "#1A9B8A",
        _ => "#00AE42",
    }
}

fn fill_button(fill: Color, text: Color, status: button::Status) -> button::Style {
    let background = match status {
        button::Status::Disabled => Color {
            a: 0.45,
            ..fill
        },
        button::Status::Pressed => Color {
            r: (fill.r * 0.85).clamp(0.0, 1.0),
            g: (fill.g * 0.85).clamp(0.0, 1.0),
            b: (fill.b * 0.85).clamp(0.0, 1.0),
            a: fill.a,
        },
        button::Status::Hovered => Color {
            r: (fill.r * 1.08).min(1.0),
            g: (fill.g * 1.08).min(1.0),
            b: (fill.b * 1.08).min(1.0),
            a: fill.a,
        },
        button::Status::Active => fill,
    };
    button::Style {
        background: Some(Background::Color(background)),
        text_color: text,
        border: Border {
            color: Color::TRANSPARENT,
            width: 0.0,
            radius: RADIUS.into(),
        },
        ..button::Style::default()
    }
}

/// Notebook tab: Prepare green / Preview teal when selected.
pub fn notebook_tab(active: bool, fill: Color, status: button::Status) -> button::Style {
    if active {
        fill_button(fill, Color::WHITE, status)
    } else {
        let bg = match status {
            button::Status::Hovered | button::Status::Pressed => INACTIVE_HOVER,
            _ => Color::TRANSPARENT,
        };
        button::Style {
            background: Some(Background::Color(bg)),
            text_color: TEXT_MUTED,
            border: Border {
                color: Color::TRANSPARENT,
                width: 0.0,
                radius: RADIUS.into(),
            },
            ..button::Style::default()
        }
    }
}

/// Slice plate: green outline, dark fill.
pub fn slice_plate(status: button::Status) -> button::Style {
    let bg = match status {
        button::Status::Hovered | button::Status::Pressed => INACTIVE_HOVER,
        _ => Color::TRANSPARENT,
    };
    button::Style {
        background: Some(Background::Color(bg)),
        text_color: PREPARE,
        border: Border {
            color: PREPARE,
            width: 1.0,
            radius: RADIUS.into(),
        },
        ..button::Style::default()
    }
}

/// Print plate: solid Studio green.
pub fn print_plate(status: button::Status) -> button::Style {
    fill_button(PREPARE, Color::WHITE, status)
}

pub fn quiet(status: button::Status) -> button::Style {
    let bg = match status {
        button::Status::Hovered | button::Status::Pressed => INACTIVE_HOVER,
        _ => INACTIVE,
    };
    button::Style {
        background: Some(Background::Color(bg)),
        text_color: TEXT,
        border: Border {
            color: CARD_BORDER,
            width: 1.0,
            radius: RADIUS.into(),
        },
        ..button::Style::default()
    }
}

pub fn process_tab(active: bool, status: button::Status) -> button::Style {
    notebook_tab(active, PREPARE, status)
}
