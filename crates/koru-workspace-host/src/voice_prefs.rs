// SPDX-License-Identifier: MPL-2.0

//! **The four dictation preferences that follow the person**, held as process
//! globals so a change made in one window is the answer every window gives.
//!
//! Language, quality, the hold-versus-toggle gesture and which microphone are
//! preferences in the ordinary sense: they are about how *you* dictate, not
//! about this folder and not about this machine.
//!
//! ## The microphone is here even though a device is a fact about a machine
//!
//! That is the one that looks wrong and is not. What is stored is a *name*
//! somebody chose, not a device — and a person who dictates through a
//! particular headset on a desktop and a laptop wants the same headset on
//! both, which is what a roaming preference gives them. A name that is not
//! present on this machine is not an error and not a correction: it falls back
//! to the system default and the settings section says so
//! ([`koru_voice::input`]). Storing it per machine would instead mean
//! *forgetting* the choice on the second machine, which is the same outcome
//! with none of the recovery. So they live in `preferences.conf` beside theme,
//! font and zoom — [`crate::Settings`] is the storage of record — and they are
//! mirrored here as atomics for exactly the reason the type roles are:
//!
//! - `App::new` seeds these from the settings it loaded;
//! - the Settings surface writes *here* when somebody clicks, because a view
//!   cannot reach `Settings`;
//! - `capture_geometry` reads them back into `Settings`, which is what saves;
//! - and the input guard in `app.rs` notices a move (mouse or keyboard) and
//!   calls `invalidate_everywhere`, so a second window's Settings tab redraws
//!   with the value that was just chosen rather than the one it was built with.
//!
//! That is the same snapshot-and-relay path theme, density and the four font
//! roles already travel, and it is the reason these are globals rather than a
//! field on the view: two Settings tabs are two views of one preference.
//!
//! ## Why here and not in `koru-draw`
//!
//! The typography globals live in `koru_draw::theme` because they are baked
//! into every layout. These are not: they change what a dictation *asks for*,
//! and their types are `koru_voice::Requested` and
//! `koru_voice_models::ModelTier`. Putting them in the draw crate would make
//! the renderer link the speech-model crate to find out which quality somebody
//! picked. This crate already links both, and it is the lowest one that both
//! the window's Settings surface and the controller-owning `App` can reach.
//!
//! ## Consent and installed state are not here
//!
//! Deliberately. Consent is a machine-local record
//! ([`koru_voice_consent`](koru_voice_consent)) and installed state is derived
//! from the verified cache
//! ([`koru_voice_models::anything_installed`]). Neither follows the person, and
//! neither is stored beside these — a preference that said *installed* would be
//! answering for a disk it cannot see.

use std::sync::atomic::{AtomicBool, AtomicU8, AtomicU64, Ordering};

/// **Re-exported, so a surface names one crate rather than three.** The
/// window's Settings page has no business linking the speech-model crate to
/// find out which quality tiers exist, and `apps/koru` deliberately does not:
/// this module is the seam, exactly as `koru_dictation` re-exports the client's
/// vocabulary for a host that routes actions.
pub use koru_voice::{InputChoice, InputDevice, InputResolution, Requested, VoiceLanguage};
pub use koru_voice_models::ModelTier;

/// `0` is [`Requested::Auto`]; a fixed language is its index in
/// [`VoiceLanguage::ALL`] plus one.
///
/// An index rather than a token, because this is read on a click path and a
/// string would need a lock. `Auto` is zero so that a default-initialised
/// atomic *is* the default preference rather than a language nobody chose.
static LANGUAGE: AtomicU8 = AtomicU8::new(0);

/// An index into [`ModelTier::ALL`], which is cheapest-first — so zero is
/// [`ModelTier::Compact`], the tier's own default and for its own reasons.
static TIER: AtomicU8 = AtomicU8::new(0);

/// `false` is push-to-talk, which is the default gesture.
static TOGGLE: AtomicBool = AtomicBool::new(false);

/// **Which microphone, by name.** A lock rather than an atomic, because unlike
/// the three above this one is a string — see [`InputChoice`] on why it may
/// not be an index. It is read on the press path, which is a key event rather
/// than a frame, so an uncontended `RwLock` read is nothing; nothing in a draw
/// pass touches it.
static INPUT: std::sync::RwLock<Option<InputChoice>> = std::sync::RwLock::new(None);

/// Bumped whenever [`set_input`] changes anything, so [`snapshot`] stays three
/// cheap loads and a fourth — a `String` compare in the mouse guard would put
/// an allocation-free-but-not-free comparison on a path that runs per event.
static INPUT_MOVED: AtomicU64 = AtomicU64::new(0);

/// The stored token for "work the language out", which is not a BCP 47 tag and
/// never will be — see [`Requested::tag`], which answers `None` for it.
pub const AUTO: &str = "auto";

pub fn language() -> Requested {
    let index = LANGUAGE.load(Ordering::Relaxed);
    match index.checked_sub(1) {
        None => Requested::Auto,
        Some(i) => VoiceLanguage::ALL
            .get(usize::from(i))
            .copied()
            .map_or(Requested::Auto, Requested::Fixed),
    }
}

pub fn set_language(requested: Requested) {
    let index = match requested {
        Requested::Auto => 0,
        Requested::Fixed(language) => VoiceLanguage::ALL
            .iter()
            .position(|l| *l == language)
            .map_or(0, |i| i as u8 + 1),
    };
    LANGUAGE.store(index, Ordering::Relaxed);
}

pub fn tier() -> ModelTier {
    let index = usize::from(TIER.load(Ordering::Relaxed));
    ModelTier::ALL.get(index).copied().unwrap_or_default()
}

pub fn set_tier(tier: ModelTier) {
    let index = ModelTier::ALL.iter().position(|t| *t == tier).unwrap_or(0);
    TIER.store(index as u8, Ordering::Relaxed);
}

/// `true` when the gesture is a toggle — press once to start, press again to
/// stop. `false` is push-to-talk.
pub fn toggle() -> bool {
    TOGGLE.load(Ordering::Relaxed)
}

pub fn set_toggle(value: bool) {
    TOGGLE.store(value, Ordering::Relaxed);
}

/// Which microphone somebody asked for. [`InputChoice::SystemDefault`] until
/// they say otherwise, which is *follow the OS* and is also what a person who
/// has never opened the picker gets.
pub fn input() -> InputChoice {
    INPUT
        .read()
        .unwrap_or_else(|error| error.into_inner())
        .clone()
        .unwrap_or_default()
}

pub fn set_input(choice: InputChoice) {
    let mut held = INPUT.write().unwrap_or_else(|error| error.into_inner());
    if held.as_ref() == Some(&choice) {
        return;
    }
    *held = Some(choice);
    INPUT_MOVED.fetch_add(1, Ordering::Relaxed);
}

/// Everything a caller has to compare to know whether any of the four moved.
///
/// One value, so the guard in `app.rs` cannot be extended for three of them and
/// silently miss the fourth — the mistake the eight-line typography snapshot
/// beside it is one bad merge away from.
pub fn snapshot() -> (u8, u8, bool, u64) {
    (
        LANGUAGE.load(Ordering::Relaxed),
        TIER.load(Ordering::Relaxed),
        TOGGLE.load(Ordering::Relaxed),
        // A counter rather than the name: the guard runs per mouse event and a
        // `String` clone-and-compare there would be paid on every move.
        INPUT_MOVED.load(Ordering::Relaxed),
    )
}

/// The token `preferences.conf` carries for a language request.
///
/// The BCP 47 tag, which is what every other boundary in this product speaks,
/// and [`AUTO`] for the one request that has no tag. **Never the label**: a
/// column holding a display string means something different in another
/// language, which is the mistake PRINCIPLES names.
pub fn language_token(requested: Requested) -> &'static str {
    requested.tag().unwrap_or(AUTO)
}

/// The request a token names.
///
/// **Anything unrecognised is [`Requested::Auto`]**, including the empty
/// string. A file written by a build that knew a ninth language must not stop
/// this one from opening, and detecting the language is the answer that is
/// wrong for nobody.
pub fn parse_language(token: &str) -> Requested {
    let token = token.trim();
    if token.is_empty() || token.eq_ignore_ascii_case(AUTO) {
        return Requested::Auto;
    }
    VoiceLanguage::matching(token).map_or(Requested::Auto, Requested::Fixed)
}

/// The tier a token names, defaulting for the same reason.
///
/// [`ModelTier::parse`] is deliberately total-free — a tier nobody declared is
/// not a tier — and the *preference* layer is where that becomes a default,
/// because a settings file is not a signed descriptor.
pub fn parse_tier(token: &str) -> ModelTier {
    ModelTier::parse(token.trim()).unwrap_or_default()
}

/// The token `preferences.conf` carries for a microphone.
///
/// [`koru_voice::SYSTEM_DEFAULT`], or the device's name verbatim. **The name,
/// never an index**, and never the label a picker draws — the reasons are
/// [`koru_voice::input`]'s and they are the whole point of the type.
pub fn input_token(choice: &InputChoice) -> &str {
    choice.token()
}

/// The choice a token names. Anything unrecognised is *follow the OS*, for the
/// same reason [`parse_language`] answers `Auto`.
pub fn parse_input(token: &str) -> InputChoice {
    InputChoice::parse(token)
}

#[cfg(test)]
mod tests;
