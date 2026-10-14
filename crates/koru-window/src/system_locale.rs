// SPDX-License-Identifier: MPL-2.0

//! What language the machine is set to, read before any window exists.
//!
//! The second question winit cannot answer, beside `system_theme`'s
//! light/dark — and here for the same reason: it is asked on the way to the
//! first frame, so it cannot wait for an event loop, and it is a platform call
//! rather than anything about a document.
//!
//! **A tag, never a decision.** This returns whatever the OS says — `ja-JP`,
//! `pt_BR.UTF-8`, `ko-KR` — and has no opinion about whether Koru has strings
//! for it. `koru_i18n::Lang::for_system` decides what to *draw*; the tag itself
//! is what gets stored on a profile, and those are two different facts. A
//! function here that returned "English" for an unsupported locale would have
//! thrown away the one that matters.
//!
//! **Three arms, not two.** `$LANG` is a *shell's* variable and not a
//! *session's*: a Mac process launched from Finder, the Dock or `launchd`
//! inherits none of `LC_ALL`, `LC_MESSAGES` or `LANG`, so the POSIX arm
//! answered `None` on every real launch and every Mac drew English whatever
//! System Settings said. macOS asks `NSLocale.preferredLanguages` instead
//! ([ADR-175](../../../docs/decisions/README.md)).

/// The OS's own locale, as close to BCP 47 as the platform gives it.
///
/// `None` when nothing answers, which is not the same as English: the absence
/// of an answer is not an answer.
pub fn system_locale() -> Option<String> {
    #[cfg(windows)]
    {
        windows_locale()
    }
    #[cfg(target_os = "macos")]
    {
        // `LC_ALL` first because nothing sets it for you — see
        // `macos_shell_override`. `LANG` and `LC_MESSAGES` are deliberately not
        // consulted here; on macOS they describe a terminal's *region*.
        macos_shell_override().or_else(macos_locale)
    }
    #[cfg(not(any(windows, target_os = "macos")))]
    {
        posix_locale()
    }
}

/// `GetUserDefaultLocaleName` — the *user's* locale, not the system's.
///
/// The distinction is real on a shared or managed machine: the system locale is
/// what the install was configured with and the user locale is what the person
/// signed in chose. It already answers in BCP 47 (`ja-JP`, `zh-Hans-CN`), so
/// nothing here reformats it.
#[cfg(windows)]
fn windows_locale() -> Option<String> {
    use windows_sys::Win32::Globalization::GetUserDefaultLocaleName;

    // `LOCALE_NAME_MAX_LENGTH`, which `windows-sys` does not re-export. Named
    // rather than rounded up: the API writes at most this many UTF-16 units
    // including the NUL, and a buffer chosen by eye is how a truncation gets
    // shipped in a language nobody on the team reads.
    const LOCALE_NAME_MAX_LENGTH: usize = 85;

    let mut buffer = [0u16; LOCALE_NAME_MAX_LENGTH];
    // SAFETY: a write of at most `buffer.len()` UTF-16 units into a buffer of
    // exactly that many, which is the contract the API documents.
    let written = unsafe { GetUserDefaultLocaleName(buffer.as_mut_ptr(), buffer.len() as i32) };
    if written <= 0 {
        return None;
    }
    // The count includes the terminating NUL.
    let tag = String::from_utf16_lossy(&buffer[..(written as usize - 1)]);
    non_empty(tag)
}

/// `NSLocale.preferredLanguages`, first entry — the person's language order.
///
/// **Why not `$LANG`.** A Mac process launched from Finder, the Dock or
/// `launchd` has no `LC_ALL`, no `LC_MESSAGES` and no `LANG`: those are set by
/// a shell's startup, and a session never had them. So the POSIX arm answered
/// `None` on every launch that was not a developer's `cargo run`, which is why
/// the whole macOS port shipped drawing English — a terminal usually *does*
/// have `LANG`, so nothing on a desk ever reproduced it.
///
/// **`NSLocale`, not `NSBundle.preferredLocalizations`.** The bundle's answer
/// is already filtered by which localizations the app ships, which is a
/// decision, and this module returns a tag. A Korean Mac has to be able to say
/// `ko-KR` here even though there are no Korean strings — that tag is what a
/// profile stores (ADR-87).
///
/// It reads `AppleLanguages` out of `NSGlobalDomain`, the same place
/// `system_theme` reads `AppleInterfaceStyle`, so it needs no `NSApplication`
/// and answers the same on the cold-start path as anywhere else — and answers
/// for an unbundled `cargo run` binary as well as for an installed `.app`.
///
/// Already BCP 47 (`en-US`, `zh-Hans-CN`, `pt-BR`), so nothing here reformats
/// it either.
#[cfg(target_os = "macos")]
fn macos_locale() -> Option<String> {
    use objc2_foundation::NSLocale;

    NSLocale::preferredLanguages()
        .firstObject()
        .and_then(|tag| non_empty(tag.to_string()))
}

/// `$LC_ALL` on macOS only, and deliberately not `$LANG`.
///
/// On macOS these two are not the same kind of value. Terminal.app and iTerm2
/// write `LANG` for you, composed from the **Region** setting rather than from
/// the preferred language — so a Mac whose interface is Japanese and whose
/// region is United States hands a shell `en_US.UTF-8`. Honouring it would make
/// a terminal launch disagree with a Finder launch about what language the
/// person reads, which is the bug this arm exists to end rather than move.
///
/// Nothing sets `LC_ALL` for you. A value in it is something a person typed, so
/// it is an override worth having: it is how the eight languages get exercised
/// without changing a System Settings pane that affects every app on the
/// machine.
#[cfg(target_os = "macos")]
fn macos_shell_override() -> Option<String> {
    MAC_OVERRIDE
        .into_iter()
        .filter_map(|name| std::env::var(name).ok())
        .find_map(non_empty)
}

/// The environment variables macOS honours, as a value rather than a literal,
/// so `only_an_explicit_lc_all_overrides_the_mac` can see the list. Adding
/// `LANG` to it re-opens the disagreement described on `macos_shell_override`.
#[cfg(target_os = "macos")]
const MAC_OVERRIDE: [&str; 1] = ["LC_ALL"];

/// `$LC_ALL`, then `$LC_MESSAGES`, then `$LANG` — the order every POSIX program
/// resolves them in.
///
/// The values are POSIX rather than BCP 47 (`ja_JP.UTF-8`), and are handed over
/// as they are: `koru_i18n` already understands the underscore and the codeset
/// suffix, and normalising here would be a second place that has to know the
/// shape.
#[cfg(not(any(windows, target_os = "macos")))]
fn posix_locale() -> Option<String> {
    ["LC_ALL", "LC_MESSAGES", "LANG"]
        .into_iter()
        .filter_map(|name| std::env::var(name).ok())
        .find_map(non_empty)
}

/// `C` and `POSIX` are the *absence* of a locale, not a language. Reporting
/// them as a tag would make an unset environment look like an answer.
///
/// **The codeset and the modifier come off before that test, not after.** A
/// shell says `C.UTF-8` at least as often as it says `C`, and `en_US.UTF-8@foo`
/// is the same language as `en_US`; a test against the whole string let
/// `C.UTF-8` through as though it named a language, and this function's answer
/// is what a profile stores. `Lang::matching` reads it as `c` and draws
/// English either way, so what the bug costs is a stored tag that says a person
/// speaks a locale that is not one. The *returned* value keeps its suffix, because
/// `koru_i18n::Lang::matching` already strips one and two places that know the
/// shape of a POSIX locale is one too many.
fn non_empty(tag: String) -> Option<String> {
    let trimmed = tag.trim();
    let bare = trimmed.split(['.', '@']).next().unwrap_or("").trim();
    if bare.is_empty() || bare == "C" || bare == "POSIX" {
        return None;
    }
    Some(trimmed.to_string())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn an_absent_locale_is_none_rather_than_a_language() {
        assert_eq!(non_empty("C".into()), None);
        assert_eq!(non_empty("POSIX".into()), None);
        assert_eq!(non_empty("   ".into()), None);
        assert_eq!(non_empty(String::new()), None);
        assert_eq!(non_empty(" ja-JP ".into()), Some("ja-JP".to_string()));
    }

    /// The codeset and the modifier are not part of the language, so the test
    /// for "no locale at all" has to see through them. `C.UTF-8` is what a
    /// container, a `launchd` job and half the CI images in the world say.
    #[test]
    fn a_codeset_or_a_modifier_does_not_disguise_the_c_locale() {
        assert_eq!(non_empty("C.UTF-8".into()), None);
        assert_eq!(non_empty("C.utf8".into()), None);
        assert_eq!(non_empty("POSIX.UTF-8".into()), None);
        assert_eq!(non_empty("C@euro".into()), None);
        assert_eq!(non_empty(".UTF-8".into()), None);
        // ...and a real locale keeps everything it arrived with.
        assert_eq!(
            non_empty("ja_JP.UTF-8".into()),
            Some("ja_JP.UTF-8".to_string())
        );
        assert_eq!(
            non_empty("de_DE@euro".into()),
            Some("de_DE@euro".to_string())
        );
    }

    /// Whatever this machine is set to, the answer has to be shaped like a tag
    /// rather than like a sentence — and it must not be a guess.
    #[test]
    fn the_machine_answers_with_a_tag_or_with_nothing() {
        if let Some(tag) = system_locale() {
            assert!(!tag.is_empty());
            assert!(!tag.contains(' '), "{tag:?} is not a locale name");
            assert!(
                tag.starts_with(|c: char| c.is_ascii_alphabetic()),
                "{tag:?}"
            );
        }
    }

    /// The whole point of M10: on a Mac there is always an answer, because
    /// `preferredLanguages` is never empty — a machine with every language
    /// removed from the list still reports the one it boots in. A `None` here
    /// means the arm is not being reached.
    ///
    /// It asserts a shape rather than a value, because the value is whatever
    /// this developer's System Settings say.
    #[cfg(target_os = "macos")]
    #[test]
    fn a_mac_always_names_a_language() {
        let tag = system_locale().expect("macOS always has a preferred language");
        let primary = tag.split(['-', '_', '.', '@']).next().unwrap_or("");
        assert!(
            primary.len() >= 2 && primary.chars().all(|c| c.is_ascii_alphabetic()),
            "{tag:?} does not begin with a language subtag"
        );
    }

    /// A tripwire rather than a proof, and deliberately so: setting a
    /// process-wide environment variable from a test races every other test in
    /// this binary, so the list is asserted instead of exercised. `LC_ALL` is
    /// the one variable nothing sets for you; `LANG`, which Terminal.app writes
    /// from the **region** and not from the language, must stay out of it or a
    /// terminal launch goes back to disagreeing with a Finder launch.
    #[cfg(target_os = "macos")]
    #[test]
    fn only_an_explicit_lc_all_overrides_the_mac() {
        assert_eq!(MAC_OVERRIDE, ["LC_ALL"]);
        // ...and a shell that exported `LC_ALL=C` has said nothing rather than
        // "the C language", so the OS still gets asked.
        assert_eq!(non_empty("C.UTF-8".into()), None);
    }
}
