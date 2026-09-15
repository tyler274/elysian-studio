//! Studio/Orca chrome tokens. Widget `style` closures use these instead of
//! iced's stock `Theme::Dark` greens/purples.

use iced::theme::Palette;
use iced::widget::overlay::menu;
use iced::widget::{button, checkbox, container, pick_list, scrollable, slider, text_input};
use iced::{Background, Border, Color, Shadow, Theme, Vector};

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
/// Overlay menus sit just below the compact header row.
pub const HEADER_DROPDOWN_TOP: f32 = 32.0;
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

pub fn dropdown() -> container::Style {
    container::Style {
        background: Some(Background::Color(CARD)),
        text_color: Some(TEXT),
        border: Border {
            color: CARD_BORDER,
            width: 1.0,
            radius: RADIUS.into(),
        },
        shadow: Shadow {
            color: Color::from_rgba(0.0, 0.0, 0.0, 0.45),
            offset: Vector::new(0.0, 4.0),
            blur_radius: 12.0,
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
        button::Status::Disabled => Color { a: 0.45, ..fill },
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

pub fn dropdown_item(status: button::Status) -> button::Style {
    let bg = match status {
        button::Status::Hovered | button::Status::Pressed => INACTIVE_HOVER,
        _ => Color::TRANSPARENT,
    };
    button::Style {
        background: Some(Background::Color(bg)),
        text_color: TEXT,
        border: Border {
            color: Color::TRANSPARENT,
            width: 0.0,
            radius: RADIUS.into(),
        },
        ..button::Style::default()
    }
}

pub fn menubar(open: bool, status: button::Status) -> button::Style {
    let bg = if open {
        INACTIVE_HOVER
    } else {
        match status {
            button::Status::Hovered | button::Status::Pressed => INACTIVE_HOVER,
            _ => Color::TRANSPARENT,
        }
    };
    button::Style {
        background: Some(Background::Color(bg)),
        text_color: TEXT,
        border: Border {
            color: Color::TRANSPARENT,
            width: 0.0,
            radius: RADIUS.into(),
        },
        ..button::Style::default()
    }
}

pub fn process_tab(active: bool, status: button::Status) -> button::Style {
    notebook_tab(active, PREPARE, status)
}

/// Window fill — keep this off iced's generated extended palette so collapsing
/// sidebar groups cannot flip the shell between Custom and Light/Dark.
pub fn window(_theme: &Theme) -> iced::theme::Style {
    iced::theme::Style {
        background_color: HEADER,
        text_color: TEXT,
    }
}

pub fn field(_theme: &Theme, status: text_input::Status) -> text_input::Style {
    let border_color = match status {
        text_input::Status::Focused { .. } => PREPARE,
        text_input::Status::Hovered => INACTIVE_HOVER,
        _ => CARD_BORDER,
    };
    text_input::Style {
        background: Background::Color(INACTIVE),
        border: Border {
            color: border_color,
            width: 1.0,
            radius: RADIUS.into(),
        },
        icon: TEXT_MUTED,
        placeholder: TEXT_MUTED,
        value: TEXT,
        selection: PREPARE,
    }
}

pub fn choice(_theme: &Theme, status: pick_list::Status) -> pick_list::Style {
    let bg = match status {
        pick_list::Status::Hovered | pick_list::Status::Opened { .. } => INACTIVE_HOVER,
        pick_list::Status::Active => INACTIVE,
    };
    pick_list::Style {
        text_color: TEXT,
        placeholder_color: TEXT_MUTED,
        handle_color: TEXT_MUTED,
        background: Background::Color(bg),
        border: Border {
            color: CARD_BORDER,
            width: 1.0,
            radius: RADIUS.into(),
        },
    }
}

pub fn menu(_theme: &Theme) -> menu::Style {
    menu::Style {
        background: Background::Color(CARD),
        border: Border {
            color: CARD_BORDER,
            width: 1.0,
            radius: RADIUS.into(),
        },
        text_color: TEXT,
        selected_text_color: Color::WHITE,
        selected_background: Background::Color(PREPARE),
        shadow: Shadow::default(),
    }
}

pub fn tick(_theme: &Theme, status: checkbox::Status) -> checkbox::Style {
    let (is_checked, hovered) = match status {
        checkbox::Status::Active { is_checked } => (is_checked, false),
        checkbox::Status::Hovered { is_checked } => (is_checked, true),
        checkbox::Status::Disabled { is_checked } => (is_checked, false),
    };
    let fill = if is_checked {
        PREPARE
    } else if hovered {
        INACTIVE_HOVER
    } else {
        INACTIVE
    };
    checkbox::Style {
        background: Background::Color(fill),
        icon_color: Color::WHITE,
        border: Border {
            color: if is_checked { PREPARE } else { CARD_BORDER },
            width: 1.0,
            radius: 2.0.into(),
        },
        text_color: Some(TEXT),
    }
}

pub fn range(_theme: &Theme, status: slider::Status) -> slider::Style {
    let handle = match status {
        slider::Status::Hovered | slider::Status::Dragged => Color {
            r: (PREPARE.r * 1.08).min(1.0),
            g: (PREPARE.g * 1.08).min(1.0),
            b: (PREPARE.b * 1.08).min(1.0),
            a: PREPARE.a,
        },
        slider::Status::Active => PREPARE,
    };
    slider::Style {
        rail: slider::Rail {
            backgrounds: (Background::Color(PREPARE), Background::Color(INACTIVE)),
            width: 4.0,
            border: Border {
                color: Color::TRANSPARENT,
                width: 0.0,
                radius: 2.0.into(),
            },
        },
        handle: slider::Handle {
            shape: slider::HandleShape::Circle { radius: 7.0 },
            background: Background::Color(handle),
            border_width: 0.0,
            border_color: Color::TRANSPARENT,
        },
    }
}

pub fn scroll(_theme: &Theme, _status: scrollable::Status) -> scrollable::Style {
    let rail = scrollable::Rail {
        background: Some(Background::Color(SIDEBAR)),
        border: Border {
            color: Color::TRANSPARENT,
            width: 0.0,
            radius: 2.0.into(),
        },
        scroller: scrollable::Scroller {
            background: Background::Color(INACTIVE_HOVER),
            border: Border {
                color: Color::TRANSPARENT,
                width: 0.0,
                radius: 2.0.into(),
            },
        },
    };
    scrollable::Style {
        container: sidebar_pane(),
        vertical_rail: rail,
        horizontal_rail: rail,
        gap: None,
        auto_scroll: scrollable::AutoScroll {
            background: Background::Color(Color { a: 0.9, ..HEADER }),
            border: Border {
                color: CARD_BORDER,
                width: 1.0,
                radius: 16.0.into(),
            },
            shadow: Shadow {
                color: Color::BLACK,
                offset: Vector::ZERO,
                blur_radius: 2.0,
            },
            icon: TEXT_MUTED,
        },
    }
}
