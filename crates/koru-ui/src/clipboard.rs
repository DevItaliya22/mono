// SPDX-License-Identifier: MPL-2.0

//! **Plain text, and nothing else.**
//!
//! A prompt field holds one line with no runs, no pills and no rows, so this is
//! the whole of what it needs from the clipboard: put a string on, take a string
//! off, flatten anything with newlines in it.
//!
//! `koru-editor` has a much larger clipboard — an in-process payload carrying
//! marks and structure, compared against the system clipboard to decide whether
//! it has gone stale. A field cannot use it and does not need to: that module
//! writes the same rendered text to the *system* clipboard on every copy, so a
//! paragraph copied out of a document still pastes into a prompt.
//!
//! The OS call goes through `koru-clipboard`, the one platform mechanism shared
//! with documents and Koru Terminal. Flattening remains here because it is a
//! one-line widget policy, not a clipboard fact.
//!
//! ## Under test it is in-process, and that is not a shortcut
//!
//! Test binaries run in parallel, so neither this crate nor `koru-editor`
//! touches the desktop clipboard in ordinary tests. Both inject the shared
//! mechanism with thread-local state. What is tested here is widget policy:
//! flatten a multi-line paste, treat blank as nothing, and preserve lines for
//! a terminal paste.

/// One injected backend per test thread: no desktop access, no lock shared with
/// another test, and the same `koru-clipboard` path production uses.
#[cfg(test)]
#[derive(Default)]
struct FixtureBackend(std::sync::Mutex<Option<String>>);

#[cfg(test)]
impl koru_clipboard::Backend for FixtureBackend {
    fn text(&self) -> Result<Option<String>, koru_clipboard::Error> {
        Ok(self
            .0
            .lock()
            .unwrap_or_else(|error| error.into_inner())
            .clone())
    }

    fn set_text(&self, text: &str) -> Result<(), koru_clipboard::Error> {
        *self.0.lock().unwrap_or_else(|error| error.into_inner()) = Some(text.to_owned());
        Ok(())
    }
}

#[cfg(test)]
thread_local! {
    static FIXTURE: koru_clipboard::Clipboard<FixtureBackend> =
        koru_clipboard::Clipboard::new(FixtureBackend::default());
}

/// Put one line of plain text on the system clipboard.
///
/// Failure is silent by design: a copy that could not reach the OS is a
/// disappointment, and a dialog about it in the middle of typing a name would
/// be worse than the disappointment.
pub fn put_plain(text: &str) {
    #[cfg(test)]
    {
        FIXTURE.with(|clipboard| {
            let _ = clipboard.set_text(text);
        });
    }
    #[cfg(not(test))]
    {
        let _ = koru_clipboard::system().set_text(text);
    }
}

/// The clipboard as one line, or `None` when it holds nothing usable.
///
/// **Newlines become spaces rather than a refusal.** Pasting three lines into a
/// one-line prompt should give you something to edit, not nothing — the same
/// rule `koru_editor::clipboard::one_line` follows, and the reason both exist
/// is that the field must not need the rich payload to obey it.
pub fn one_line() -> Option<String> {
    let text = plain()?;

    let flat = text.replace(['\n', '\r', '\t'], " ");
    (!flat.trim().is_empty()).then_some(flat)
}

/// The clipboard's complete plain-text payload, preserving line endings.
///
/// Terminal paste is the reason this is separate from [`one_line`]: flattening
/// a shell script changes what the process receives. Both accessors still use
/// this one OS clipboard implementation.
pub fn plain() -> Option<String> {
    #[cfg(test)]
    let text = FIXTURE.with(|clipboard| match clipboard.text() {
        koru_clipboard::TextPayload::Text(text) => Some(text),
        koru_clipboard::TextPayload::NonText | koru_clipboard::TextPayload::Unavailable => None,
    })?;
    #[cfg(not(test))]
    let text = match koru_clipboard::system().text() {
        koru_clipboard::TextPayload::Text(text) => text,
        koru_clipboard::TextPayload::NonText | koru_clipboard::TextPayload::Unavailable => {
            return None;
        }
    };

    (!text.is_empty()).then_some(text)
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The rule a one-line field lives by: a multi-line paste is flattened
    /// rather than dropped.
    #[test]
    fn a_multi_line_copy_comes_back_as_one_line() {
        put_plain("one\r\ntwo\tthree");
        assert_eq!(one_line().as_deref(), Some("one  two three"));
    }

    #[test]
    fn whitespace_alone_is_nothing_to_paste() {
        put_plain("   \n  ");
        assert_eq!(one_line(), None);
    }

    #[test]
    fn terminal_plain_text_preserves_lines_and_tabs() {
        put_plain("one\r\ntwo\tthree");
        assert_eq!(plain().as_deref(), Some("one\r\ntwo\tthree"));
    }
}
