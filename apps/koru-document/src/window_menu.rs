// SPDX-License-Identifier: MPL-2.0

//! The window-menu button's menu, for a document opened in its own window.
//!
//! **No *Send to Tray* here.** The tray takes the whole application down — the
//! icon is one icon and clicking it brings *every* window back — so offering it
//! from one document window says "put this window away" and does something
//! considerably larger. It stays on the container's menu, where the subject
//! genuinely is Koru.
//!
//! Otherwise the same rows the container's window menu offers — pin, minimise,
//! maximise, close — each with the chord that does the same thing, because
//! teaching those chords is why the four unlabelled buttons became a menu.
//!
//! **This is a second implementation of one menu, and that is deliberate.** The
//! container's lives in `apps/koru/src/mainview/context_menu.rs`, and this
//! crate cannot see it: `apps/koru` depends on `apps/koru-document`, not the
//! other way round. What the two share is the part that must not drift — the
//! button's geometry (`koru_ui::caption::controls`), the glyphs, and
//! the [`koru_window::WindowRequest`]s the rows raise. The rows themselves are
//! six lines each; the abstraction that would unify them is the one
//! `docs/contributing/techdebt/README.md` says not to build yet, because `menu.rs` and
//! `context_menu.rs` deliberately differ in *when* their rows are fixed.

use koru_draw::icons;

use crate::menu::{Chord, Menu, Row};

/// How wide the panel draws. Fixed rather than measured, the way every menu in
/// this crate is — wide enough for *Turn Off Always on Top* and its chord.
const WIDTH: f32 = 236.0;

/// What choosing a row does. Each maps to exactly one window request.
#[derive(Copy, Clone, PartialEq, Eq, Debug)]
pub enum Choice {
    TogglePin,
    Minimise,
    ToggleMaximise,
    SnapLeft,
    SnapRight,
    Close,
}

/// The rows, paired with what each one does.
///
/// One list rather than two parallel ones, for the reason `page_menu` gives:
/// two rows read off the window's state, so an index is not a constant.
pub fn rows(pinned: bool, maximized: bool) -> Vec<(Row, Option<Choice>)> {
    vec![
        (
            Row::item(if pinned {
                "Turn Off Always on Top"
            } else {
                "Always on Top"
            })
            .with_icon(icons::PUSH_PIN)
            .with_shortcut(Chord::Id(koru_keys::ShortcutId::PinWindow))
            .checked(pinned),
            Some(Choice::TogglePin),
        ),
        (
            // **Not `Win+M`** — that minimises *every* window on the desktop,
            // not this one. `Win+Down` is the one that acts on the active
            // window. The container's menu says the same; this copy drifted.
            Row::item("Minimize")
                .with_icon(icons::WINDOW_MINIMISE)
                .with_optional_shortcut(Chord::windows_only("Win+Down")),
            Some(Choice::Minimise),
        ),
        if maximized {
            (
                Row::item("Restore")
                    .with_icon(icons::WINDOW_RESTORE)
                    .with_optional_shortcut(Chord::windows_only("Win+Down")),
                Some(Choice::ToggleMaximise),
            )
        } else {
            (
                Row::item("Maximize")
                    .with_icon(icons::SQUARE)
                    .with_optional_shortcut(Chord::windows_only("Win+Up")),
                Some(Choice::ToggleMaximise),
            )
        },
        // Windows-only, for the reason `koru_window::snap` gives: there is no
        // work-area call to make elsewhere, and half-screen tiling is the
        // window manager's business. Absent rather than dimmed — a row that can
        // never be enabled on your machine is not information.
        #[cfg(windows)]
        (
            Row::item("Snap Left")
                .with_icon(icons::ALIGN_LEFT)
                .with_optional_shortcut(Chord::windows_only("Win+Left")),
            Some(Choice::SnapLeft),
        ),
        #[cfg(windows)]
        (
            Row::item("Snap Right")
                .with_icon(icons::ALIGN_RIGHT)
                .with_optional_shortcut(Chord::windows_only("Win+Right")),
            Some(Choice::SnapRight),
        ),
        (
            // **`Ctrl+W`, not `Alt+F4`** — though both close this window. Each
            // menu names the chord that does *this* thing in *this* window: in
            // the container `Ctrl+W` closes a tab, so its Close Window row says
            // `Alt+F4`; here there are no tabs and `Ctrl+W` closes the window
            // (`lib.rs`, the `w` arm). Alt+F4 needs no teaching — every window
            // on the platform answers it — and the shortcut column exists to
            // teach the one you would not otherwise find.
            Row::item("Close Window")
                .with_icon(icons::WINDOW_CLOSE)
                .with_shortcut(Chord::Id(koru_keys::ShortcutId::CloseDocumentWindow)),
            Some(Choice::Close),
        ),
    ]
}

/// Just the rows, for the geometry.
pub fn shape(pinned: bool, maximized: bool) -> Vec<Row> {
    rows(pinned, maximized)
        .into_iter()
        .map(|(r, _)| r)
        .collect()
}

/// What row `i` does, or `None` for a row that is furniture.
pub fn choice_at(pinned: bool, maximized: bool, i: usize) -> Option<Choice> {
    rows(pinned, maximized).get(i).and_then(|(_, c)| *c)
}

/// Open it under the caption's window-menu button.
///
/// [`Menu::panel`] hangs left-aligned under the anchor and then clamps inside
/// the viewport, so a button in the top-right corner gets a panel tucked against
/// the right edge rather than one running off it.
pub fn open(button: koru_draw::geometry::Rect) -> Menu {
    Menu::new(button, WIDTH)
}

#[cfg(test)]
mod tests;
