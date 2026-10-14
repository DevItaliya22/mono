// SPDX-License-Identifier: MPL-2.0

//! **The Terminal settings, where Koru's terminal tabs can read them.**
//!
//! The Settings page's Terminal section patches `TerminalSettings` at the
//! authority, but until this module nothing in Koru *read* it back: the one
//! reader was the standalone Koru Terminal, since deleted, so every session tab
//! opened with the built-in default cursor whatever Settings said.
//!
//! One process-wide value, fed from two places and read by every tab:
//!
//! - **a reader link per window** ([`Reader`]), so a tab opened before anybody
//!   visits Settings already has the stored cursor, and a change made in
//!   another window arrives;
//! - **the Settings tab itself** ([`adopt`]), so a press shows in an open
//!   terminal at once rather than after the authority's round trip.
//!
//! A version counter lets a tab re-apply only when something moved — the
//! pattern the spelling preferences use (ADR-252).

use std::sync::Mutex;
use std::sync::atomic::{AtomicU64, Ordering};

use koru_draw::theme::CaretColor;
use koru_session_view::{
    TerminalCursorBlink, TerminalCursorHeight, TerminalCursorPreferences, TerminalCursorWidth,
};
use koru_settings::TerminalSettings;

static CURRENT: Mutex<Option<TerminalSettings>> = Mutex::new(None);
static VERSION: AtomicU64 = AtomicU64::new(0);

/// Take a Terminal section. Bumps the version only when it differs.
pub fn adopt(section: &TerminalSettings) {
    let mut current = CURRENT.lock().unwrap_or_else(|held| held.into_inner());
    if current.as_ref() != Some(section) {
        *current = Some(section.clone());
        VERSION.fetch_add(1, Ordering::Relaxed);
    }
}

/// Moves when the section does, **or when the editor caret's colour does**:
/// a cursor matching the editor caret takes that colour, so a tab has to
/// re-read when either changes.
pub fn version() -> u64 {
    let editor = koru_draw::theme::caret_color().key();
    let editor = editor.bytes().fold(0u64, |hash, b| {
        hash.wrapping_mul(31).wrapping_add(u64::from(b))
    });
    VERSION.load(Ordering::Relaxed).wrapping_mul(1_000_003) ^ editor
}

/// The cursor a terminal tab should draw: the stored one, or the built-in
/// default until a section has arrived.
pub fn cursor() -> TerminalCursorPreferences {
    CURRENT
        .lock()
        .unwrap_or_else(|held| held.into_inner())
        .as_ref()
        .map_or_else(TerminalCursorPreferences::default, cursor_of)
}

pub fn appearance() -> koru_session_view::TerminalAppearance {
    let current = CURRENT.lock().unwrap_or_else(|held| held.into_inner());
    let section = current.clone().unwrap_or_default();
    appearance_of(&section, koru_draw::theme::system_is_dark())
}

fn appearance_of(
    section: &TerminalSettings,
    system_dark: bool,
) -> koru_session_view::TerminalAppearance {
    use koru_settings::{FontFamily, ThemeChoice};
    koru_session_view::TerminalAppearance {
        dark: match section.theme {
            ThemeChoice::System => system_dark,
            ThemeChoice::Light => false,
            ThemeChoice::Dark => true,
        },
        font: koru_draw::theme::FontChoice::parse(
            section
                .font
                .family()
                .unwrap_or(FontFamily::JetBrainsMono)
                .token(),
        ),
        zoom: section.zoom.get(),
    }
}

/// The settings vocabulary to the renderer's. Both sides spell the values
/// with the same tokens, so the conversion goes through them.
pub fn cursor_of(section: &TerminalSettings) -> TerminalCursorPreferences {
    cursor_matching(section, koru_draw::theme::caret_color())
}

/// **Matching the editor caret covers its colour as well as its shape**: a
/// cursor whose width and height follow the editor's takes the editor's
/// colour too, so Settings can fold the colour row away while it matches.
/// Blink is the terminal's own either way — the editor caret has none to
/// follow.
fn cursor_matching(section: &TerminalSettings, editor: CaretColor) -> TerminalCursorPreferences {
    let matching = section.cursor_width == koru_settings::TerminalCursorWidth::Application
        && section.cursor_height == koru_settings::TerminalCursorHeight::Application;
    TerminalCursorPreferences {
        color: if matching {
            editor
        } else {
            CaretColor::parse(section.cursor_color.token())
        },
        width: TerminalCursorWidth::parse(section.cursor_width.token()),
        height: TerminalCursorHeight::parse(section.cursor_height.token()),
        blink: TerminalCursorBlink::parse(section.cursor_blink.token()),
    }
}

/// A window's read of the Terminal section.
pub struct Reader {
    links: koru_settings_ui::SettingsLinks,
}

impl Reader {
    pub fn new() -> Self {
        // A dependency is never compiled with `cfg(test)`, so this binary's
        // suite must not dial whatever Gateway the machine has running.
        let links = if cfg!(test) {
            koru_settings_ui::SettingsLinks::inert(false, true)
        } else {
            koru_settings_ui::SettingsLinks::terminal_only(
                koru_channel::segment(),
                koru_workspace_host::cli::root_from_args(),
            )
        };
        Self { links }
    }

    /// Take whatever the link has. Never blocks.
    pub fn poll(&mut self) -> bool {
        match self.links.poll_terminal_section() {
            Some(section) => {
                let before = version();
                adopt(&section);
                version() != before
            }
            None => false,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use koru_settings::{CaretColorChoice, TerminalCursorBlink as Blink};

    #[test]
    fn terminal_appearance_uses_its_own_settings_and_resolves_system() {
        use koru_settings::{FontFamily, FontSetting, ThemeChoice, Zoom};
        let mut section = TerminalSettings {
            theme: ThemeChoice::System,
            font: FontSetting::Chosen(FontFamily::SourceCodePro),
            zoom: Zoom::new(1.25).unwrap(),
            ..TerminalSettings::default()
        };
        assert!(appearance_of(&section, true).dark);
        assert!(!appearance_of(&section, false).dark);
        section.theme = ThemeChoice::Light;
        let appearance = appearance_of(&section, true);
        assert!(!appearance.dark);
        assert_eq!(appearance.font, koru_draw::theme::FontChoice::SourceCodePro);
        assert_eq!(appearance.zoom, 1.25);
        section.theme = ThemeChoice::Dark;
        assert!(appearance_of(&section, false).dark);
    }

    #[test]
    fn a_cursor_matching_the_editor_caret_takes_its_colour() {
        let section = TerminalSettings {
            cursor_color: CaretColorChoice::Amber,
            cursor_width: koru_settings::TerminalCursorWidth::Application,
            cursor_height: koru_settings::TerminalCursorHeight::Application,
            ..TerminalSettings::default()
        };
        let editor = CaretColor::parse("blue");
        assert_eq!(cursor_matching(&section, editor).color, editor);
        let own = TerminalSettings {
            cursor_width: koru_settings::TerminalCursorWidth::Thin,
            ..section
        };
        assert_eq!(cursor_matching(&own, editor).color.key(), "amber");
    }

    #[test]
    fn every_stored_cursor_value_reaches_the_renderer() {
        for color in CaretColorChoice::ALL {
            // Its own shape, so its own colour.
            let section = TerminalSettings {
                cursor_color: *color,
                cursor_width: koru_settings::TerminalCursorWidth::Thin,
                ..TerminalSettings::default()
            };
            assert_eq!(cursor_of(&section).color.key(), color.token(), "{color:?}");
        }
        let section = TerminalSettings {
            cursor_width: koru_settings::TerminalCursorWidth::X4,
            cursor_height: koru_settings::TerminalCursorHeight::Tall,
            cursor_blink: Blink::Off,
            ..TerminalSettings::default()
        };
        let cursor = cursor_of(&section);
        assert_eq!(cursor.width, TerminalCursorWidth::X4);
        assert_eq!(cursor.height, TerminalCursorHeight::Tall);
        assert_eq!(cursor.blink, TerminalCursorBlink::Off);
    }
}
