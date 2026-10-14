// SPDX-License-Identifier: MPL-2.0

//! What the window actually received, on demand.
//!
//! ```sh
//! KORU_INPUT_DEBUG=1 ./koru.sh run
//! ```
//!
//! Off, this is a load of a `bool` and one branch. On, it prints one line per
//! key press and per mouse button — **before** anything dispatches, so a chord
//! that reaches no handler still shows up. That is the whole point: a binding
//! that "does not work" is either a key that never arrived, a key that arrived
//! under a name nobody matched, or a handler that declined it, and only the
//! first two are visible from here.
//!
//! It sits beside the whole `WindowEvent` match rather than inside the arms
//! because the arms are the thing under suspicion: a `MouseButton` no arm names
//! falls into the catch-all and vanishes, which is exactly the case worth
//! seeing. Every button is logged, including `Other(n)`.
//!
//! Output goes to stderr, so a debug build shows it in the console it keeps. A
//! release build has no console (`windows_subsystem = "windows"`), so run a
//! debug build — which `./koru.sh run` already does.
//!
//! ## Why it is in `koru-ui`
//!
//! It used to be re-exported by the workspace host, and every reusable surface
//! that wanted to say what key it had received therefore had to link a
//! workspace to say it.
//! What it prints is a `Key`, a `ModifiersState` and the answer
//! [`ctrl_letter`](super::ctrl_letter) gave — a description of exactly what
//! its parent module normalises. The workspace host imports it directly under
//! the local name `inputdebug`; reusable surfaces keep the owning path visible.

use std::fmt;
use std::sync::OnceLock;

use winit::event::{ElementState, MouseButton, WindowEvent};
use winit::keyboard::{Key, ModifiersState, NamedKey, PhysicalKey};

pub fn enabled() -> bool {
    static ON: OnceLock<bool> = OnceLock::new();
    *ON.get_or_init(|| koru_env::switch(koru_env::name::KORU_INPUT_DEBUG))
}

pub fn write(args: fmt::Arguments<'_>) {
    if enabled() {
        eprintln!("{args}");
    }
}

/// Say so at startup, beside the profile line.
///
/// A switch that changes what the program prints has to admit it is on, or a log
/// pasted into a bug report is ambiguous about whether the *absence* of a line
/// means "the key never arrived" or "logging was never enabled". That difference
/// is the entire diagnostic value of this module.
pub fn announce() {
    if enabled() {
        println!("[Koru] KORU_INPUT_DEBUG is on - input dispatch stages are logged");
        println!("[Koru]   customer text is redacted; lengths and control code points remain");
    }
}

/// Which modifiers are down, as a short string — `ctrl+shift` rather than the
/// `ModifiersState` debug spew, so a line stays readable.
pub fn mods_of(mods: ModifiersState) -> String {
    let mut out = Vec::new();
    for (held, name) in [
        (mods.control_key(), "ctrl"),
        (mods.shift_key(), "shift"),
        (mods.alt_key(), "alt"),
        (mods.super_key(), "super"),
    ] {
        if held {
            out.push(name);
        }
    }
    if out.is_empty() {
        "none".to_string()
    } else {
        out.join("+")
    }
}

/// A privacy-safe logical-key description. Printable customer text is never
/// copied into a trace that may be pasted into a bug report.
pub fn key_of(key: &Key) -> String {
    match key {
        Key::Named(named) => format!("Named({named:?})"),
        Key::Character(value) if value.chars().all(char::is_control) => {
            let codes = value
                .chars()
                .map(|ch| format!("U+{:04X}", ch as u32))
                .collect::<Vec<_>>()
                .join(",");
            format!("Character(control:{codes})")
        }
        Key::Character(value) => {
            format!("Character(printable:{} scalar(s))", value.chars().count())
        }
        Key::Unidentified(code) => format!("Unidentified({code:?})"),
        Key::Dead(value) => format!("Dead({value:?})"),
    }
}

/// A privacy-safe `KeyEvent::text` description. Control encodings are the fact
/// needed for this bug; printable content is represented only by its length.
pub fn text_of(text: Option<&str>) -> String {
    match text {
        None => "none".to_string(),
        Some(value) if value.chars().all(char::is_control) => {
            let codes = value
                .chars()
                .map(|ch| format!("U+{:04X}", ch as u32))
                .collect::<Vec<_>>()
                .join(",");
            format!("control:{codes}")
        }
        Some(value) => format!("printable:{} scalar(s)", value.chars().count()),
    }
}

/// Physical letter keys can reconstruct ordinary typed text. They are useful
/// for chords, but redact them for unmodified printable input.
pub fn physical_of(physical: PhysicalKey, logical: &Key, mods: ModifiersState) -> String {
    let command = mods.control_key() || mods.super_key() || mods.alt_key();
    if !command && matches!(logical, Key::Character(value) if !value.chars().all(char::is_control))
    {
        "<redacted printable key>".to_string()
    } else {
        format!("{physical:?}")
    }
}

/// Print a key press or a mouse button, if the switch is on.
///
/// Takes the whole event so the call site is one line at the top of the match.
/// Everything that is not an input is ignored — a resize per pixel of a window
/// drag would bury the two things this is for.
pub fn observe(event: &WindowEvent, mods: ModifiersState) {
    if !enabled() {
        return;
    }
    match event {
        WindowEvent::KeyboardInput { event, .. } => {
            if event.state != ElementState::Pressed {
                return;
            }
            // Holding Ctrl auto-repeats; logging each one buries the click
            // that actually mattered and looks like Ctrl is stuck.
            if event.repeat
                && matches!(
                    event.logical_key,
                    Key::Named(
                        NamedKey::Control
                            | NamedKey::Shift
                            | NamedKey::Alt
                            | NamedKey::Super
                            | NamedKey::Meta
                            | NamedKey::Hyper
                    )
                )
            {
                return;
            }
            // `ctrl_letter` is what every chord table matches on, so its answer
            // is more use than the raw logical key: `Ctrl+L` arrives as the
            // control code 0x0C and only becomes `'l'` here.
            let letter = super::ctrl_letter(&event.logical_key, event.text.as_deref());
            eprintln!(
                "[Koru][input] os press physical={} logical={} text={} reported_mods={} ctrl_letter={:?}",
                physical_of(event.physical_key, &event.logical_key, mods),
                key_of(&event.logical_key),
                text_of(event.text.as_deref()),
                mods_of(mods),
                letter,
            );
        }
        // Focus transitions, because a press that arrives across one is the
        // whole story in a window-switch bug: an Alt+Tab Tab reaching the
        // editor as a *bare* key is only explainable by where it sits relative
        // to the focus change. No text to redact.
        WindowEvent::Focused(focused) => {
            eprintln!("[Koru][input] focus {focused}");
        }
        WindowEvent::MouseInput { state, button, .. } => {
            // **Both edges**, because a button that reports a press and no
            // release (or the reverse) is its own kind of broken, and the thumb
            // buttons are the ones being checked.
            let named = match button {
                MouseButton::Back => " (Back — walks history backwards)",
                MouseButton::Forward => " (Forward — walks history forwards)",
                MouseButton::Other(_) => " (unmapped by winit — no handler sees this)",
                _ => "",
            };
            eprintln!("[Koru][input] mouse {button:?} {state:?}{named}");
        }
        _ => {}
    }
}

/// Log a line when `KORU_INPUT_DEBUG` is on.
///
/// For the half of a gesture the event log cannot show: what a *handler* did
/// with the press it was given. A window drag that never starts and a window
/// drag that starts and is cancelled look identical from the event stream, and
/// only one of them is winit declining the call.
pub fn note(line: impl fmt::Display) {
    write(format_args!("{line}"));
}

/// Log whether a platform call this app cannot verify actually happened.
///
/// **The failure this exists for is silence.** `drag_window` and
/// `drag_resize_window` return `Result`, winit implements neither on every
/// platform, and both call sites used to discard the error with `let _ =` — so
/// a gesture that could never work looked exactly like one that was simply not
/// tried. `drag_resize_window` is unimplemented on macOS and swallowed every
/// press in a 6 px band for as long as nobody looked.
pub fn note_result<T, E: fmt::Debug>(what: &str, outcome: &Result<T, E>) {
    if !enabled() {
        return;
    }
    match outcome {
        Ok(_) => write(format_args!("{what} -> ok")),
        Err(e) => write(format_args!("{what} -> REFUSED BY WINIT: {e:?}")),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    // The strict-allowlist rule this module used to carry is gone: every
    // switch now obeys `koru_env::switch`'s one rule, pinned by that crate's
    // own `the_one_truthiness_rule` test.

    #[test]
    fn printable_customer_text_is_redacted() {
        assert_eq!(
            key_of(&Key::Character("private".into())),
            "Character(printable:7 scalar(s))"
        );
        assert_eq!(text_of(Some("private")), "printable:7 scalar(s)");
    }

    #[test]
    fn control_encodings_remain_diagnostic() {
        assert_eq!(
            key_of(&Key::Character("\u{8}".into())),
            "Character(control:U+0008)"
        );
        assert_eq!(text_of(Some("\u{7f}")), "control:U+007F");
    }

    #[test]
    fn an_unmodified_printable_physical_key_is_redacted() {
        assert_eq!(
            physical_of(
                PhysicalKey::Code(winit::keyboard::KeyCode::KeyP),
                &Key::Character("p".into()),
                ModifiersState::empty(),
            ),
            "<redacted printable key>"
        );
        assert_eq!(
            physical_of(
                PhysicalKey::Code(winit::keyboard::KeyCode::Backspace),
                &Key::Named(winit::keyboard::NamedKey::Backspace),
                ModifiersState::CONTROL,
            ),
            "Code(Backspace)"
        );
    }
}
