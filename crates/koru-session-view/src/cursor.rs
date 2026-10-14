// SPDX-License-Identifier: MPL-2.0

//! Terminal-owned cursor presentation preferences.
//!
//! `Application` preserves the PTY's authored shape/blink behavior. Explicit
//! dimensions are a user override, while `Hidden` remains authoritative.

use koru_draw::theme::{CaretColor, CaretHeight, CaretWidth};

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum TerminalCursorWidth {
    #[default]
    Application,
    Thin,
    Medium,
    Thick,
    X4,
    X8,
}

impl TerminalCursorWidth {
    pub const ALL: [Self; 6] = [
        Self::Application,
        Self::Thin,
        Self::Medium,
        Self::Thick,
        Self::X4,
        Self::X8,
    ];

    pub fn key(self) -> &'static str {
        match self {
            Self::Application => "application",
            Self::Thin => "thin",
            Self::Medium => "medium",
            Self::Thick => "thick",
            Self::X4 => "4x",
            Self::X8 => "8x",
        }
    }

    pub fn parse(value: &str) -> Self {
        match value.trim().to_ascii_lowercase().as_str() {
            "thin" => Self::Thin,
            "medium" => Self::Medium,
            "thick" => Self::Thick,
            "4x" | "x4" => Self::X4,
            "8x" | "x8" => Self::X8,
            _ => Self::Application,
        }
    }

    pub(crate) fn pixels(self) -> Option<f64> {
        let width = match self {
            Self::Application => return None,
            Self::Thin => CaretWidth::Thin,
            Self::Medium => CaretWidth::Medium,
            Self::Thick => CaretWidth::Thick,
            Self::X4 => CaretWidth::X4,
            Self::X8 => CaretWidth::X8,
        };
        Some(f64::from(width.px()))
    }
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum TerminalCursorHeight {
    #[default]
    Application,
    Short,
    Medium,
    Tall,
}

impl TerminalCursorHeight {
    pub const ALL: [Self; 4] = [Self::Application, Self::Short, Self::Medium, Self::Tall];

    pub fn key(self) -> &'static str {
        match self {
            Self::Application => "application",
            Self::Short => "short",
            Self::Medium => "medium",
            Self::Tall => "tall",
        }
    }

    pub fn parse(value: &str) -> Self {
        match value.trim().to_ascii_lowercase().as_str() {
            "short" => Self::Short,
            "medium" => Self::Medium,
            "tall" => Self::Tall,
            _ => Self::Application,
        }
    }

    pub(crate) fn scale(self) -> Option<f64> {
        match self {
            Self::Application => None,
            Self::Short => Some(CaretHeight::Short.scale()),
            Self::Medium => Some(CaretHeight::Medium.scale()),
            Self::Tall => Some(CaretHeight::Tall.scale()),
        }
    }
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum TerminalCursorBlink {
    #[default]
    Application,
    On,
    Off,
}

impl TerminalCursorBlink {
    pub const ALL: [Self; 3] = [Self::Application, Self::On, Self::Off];

    pub fn key(self) -> &'static str {
        match self {
            Self::Application => "application",
            Self::On => "on",
            Self::Off => "off",
        }
    }

    pub fn parse(value: &str) -> Self {
        match value.trim().to_ascii_lowercase().as_str() {
            "on" | "true" | "1" => Self::On,
            "off" | "false" | "0" => Self::Off,
            _ => Self::Application,
        }
    }

    pub fn enabled(self, application: bool) -> bool {
        match self {
            Self::Application => application,
            Self::On => true,
            Self::Off => false,
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct TerminalCursorPreferences {
    pub color: CaretColor,
    pub width: TerminalCursorWidth,
    pub height: TerminalCursorHeight,
    pub blink: TerminalCursorBlink,
}

impl Default for TerminalCursorPreferences {
    fn default() -> Self {
        Self {
            color: CaretColor::Default,
            width: TerminalCursorWidth::Application,
            height: TerminalCursorHeight::Application,
            // Shells generally inherit VTE's non-blinking default. Koru's
            // terminal cursor is visible as an editing affordance by default;
            // users can still defer to an application's DECSCUSR choice.
            blink: TerminalCursorBlink::On,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn persisted_tokens_round_trip_every_cursor_choice() {
        for width in TerminalCursorWidth::ALL {
            assert_eq!(TerminalCursorWidth::parse(width.key()), width);
        }
        for height in TerminalCursorHeight::ALL {
            assert_eq!(TerminalCursorHeight::parse(height.key()), height);
        }
        for blink in TerminalCursorBlink::ALL {
            assert_eq!(TerminalCursorBlink::parse(blink.key()), blink);
        }
    }

    #[test]
    fn application_blink_defers_to_the_terminal() {
        assert!(TerminalCursorBlink::Application.enabled(true));
        assert!(!TerminalCursorBlink::Application.enabled(false));
        assert!(TerminalCursorBlink::On.enabled(false));
        assert!(!TerminalCursorBlink::Off.enabled(true));
    }
}
