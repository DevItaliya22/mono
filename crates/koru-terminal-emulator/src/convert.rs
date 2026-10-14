// SPDX-License-Identifier: MPL-2.0

use alacritty_terminal::term::cell::{Cell, Flags};
use alacritty_terminal::vte::ansi::{Color, CursorShape as AlacrittyCursorShape, NamedColor};
use koru_session_protocol::{
    CellStyle, CellWidth, ColumnIndex, CursorShape, TerminalCell, TerminalColor,
};

pub(crate) fn cell(column: usize, source: &Cell) -> TerminalCell {
    let wide_spacer = source
        .flags
        .intersects(Flags::WIDE_CHAR_SPACER | Flags::LEADING_WIDE_CHAR_SPACER);
    let hidden = source.flags.contains(Flags::HIDDEN);
    let mut text = String::new();
    if !wide_spacer && !hidden {
        text.push(source.c);
        if let Some(combining) = source.zerowidth() {
            text.extend(combining);
        }
    }

    TerminalCell {
        column: ColumnIndex(column as u16),
        text,
        width: if wide_spacer {
            CellWidth::Zero
        } else if source.flags.contains(Flags::WIDE_CHAR) {
            CellWidth::Two
        } else {
            CellWidth::One
        },
        style: CellStyle {
            foreground: color(source.fg, true),
            background: color(source.bg, false),
            bold: source.flags.contains(Flags::BOLD),
            dim: source.flags.contains(Flags::DIM),
            italic: source.flags.contains(Flags::ITALIC),
            underline: source.flags.intersects(Flags::ALL_UNDERLINES),
            strikeout: source.flags.contains(Flags::STRIKEOUT),
            inverse: source.flags.contains(Flags::INVERSE),
        },
    }
}

fn color(source: Color, foreground: bool) -> TerminalColor {
    match source {
        Color::Indexed(index) => TerminalColor::Indexed { index },
        Color::Spec(rgb) => TerminalColor::Rgb {
            red: rgb.r,
            green: rgb.g,
            blue: rgb.b,
        },
        Color::Named(named) => named_color(named, foreground),
    }
}

fn named_color(source: NamedColor, foreground: bool) -> TerminalColor {
    let index = match source {
        NamedColor::Black | NamedColor::DimBlack => Some(0),
        NamedColor::Red | NamedColor::DimRed => Some(1),
        NamedColor::Green | NamedColor::DimGreen => Some(2),
        NamedColor::Yellow | NamedColor::DimYellow => Some(3),
        NamedColor::Blue | NamedColor::DimBlue => Some(4),
        NamedColor::Magenta | NamedColor::DimMagenta => Some(5),
        NamedColor::Cyan | NamedColor::DimCyan => Some(6),
        NamedColor::White | NamedColor::DimWhite => Some(7),
        NamedColor::BrightBlack => Some(8),
        NamedColor::BrightRed => Some(9),
        NamedColor::BrightGreen => Some(10),
        NamedColor::BrightYellow => Some(11),
        NamedColor::BrightBlue => Some(12),
        NamedColor::BrightMagenta => Some(13),
        NamedColor::BrightCyan => Some(14),
        NamedColor::BrightWhite => Some(15),
        NamedColor::Foreground
        | NamedColor::BrightForeground
        | NamedColor::DimForeground
        | NamedColor::Cursor => None,
        NamedColor::Background => return TerminalColor::DefaultBackground,
    };

    index.map_or_else(
        || {
            if foreground {
                TerminalColor::DefaultForeground
            } else {
                TerminalColor::DefaultBackground
            }
        },
        |index| TerminalColor::Indexed { index },
    )
}

pub(crate) fn cursor_shape(source: AlacrittyCursorShape) -> CursorShape {
    match source {
        AlacrittyCursorShape::Block | AlacrittyCursorShape::HollowBlock => CursorShape::Block,
        AlacrittyCursorShape::Underline => CursorShape::Underline,
        AlacrittyCursorShape::Beam => CursorShape::Beam,
        AlacrittyCursorShape::Hidden => CursorShape::Hidden,
    }
}
