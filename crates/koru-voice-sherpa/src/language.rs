// SPDX-License-Identifier: MPL-2.0

//! **How a language reaches a sherpa stream, which is not how it reaches most
//! recognisers.**
//!
//! Nemotron 3.5 ASR Streaming is a *prompted* transducer: its encoder metadata
//! carries a `prompt_dictionary` and an `auto_prompt_id`, and sherpa exposes
//! that as a **per-stream option**, not a field on the recogniser's config:
//!
//! ```text
//! stream.set_option("language", "ja");   // forced
//! stream.set_option("language", "auto"); // detect — and so does ""
//! ```
//!
//! Three consequences the adapters depend on.
//!
//! **The option is asked for, never assumed.** `stream.has_option("language")`
//! answers whether this runtime exposes the per-stream *override*. Setting an
//! unavailable option would be a silent no-op, so the adapter probes once at
//! construction before forcing a language. That answer is not the same as
//! auto-detection: multilingual Nemotron uses its auto prompt when the option
//! is omitted, which is exactly the ordinary `Requested::Auto` path.
//!
//! **The value is a bare subtag, not a BCP 47 tag.** The upstream example is
//! `--language=ja`, and `zh-Hans` is not a value the prompt dictionary carries.
//! [`option_value`] is that mapping and it is deliberately *lossy in one
//! direction only*: `VoiceLanguage::PtBr` becomes `pt`, so a caller cannot
//! conclude from the option that European Portuguese was accepted.
//!
//! **`auto` is a real value here and is still not a
//! [`koru_voice::VoiceLanguage`].** `koru-voice` splits [`Requested`] from
//! `VoiceLanguage` for exactly this reason; this module is where the split pays
//! off, because the string sherpa wants for "work it out" is one no language
//! could ever be named.

use koru_voice::{Requested, VoiceLanguage};

/// The per-stream option key. One spelling, from sherpa's own C API docs.
pub const OPTION_KEY: &str = "language";

/// What "work it out from the audio" is spelled as.
///
/// The upstream README says an empty string means the same thing. We send
/// `auto` rather than `""` because an empty option value is indistinguishable
/// from a value that failed to be written, and a debugging session that turns
/// on `KORU_INPUT_DEBUG`-style tracing should be able to see which was meant.
pub const AUTO: &str = "auto";

/// The value to hand `set_option(OPTION_KEY, ..)` for one request.
pub fn option_value(requested: Requested) -> &'static str {
    match requested {
        Requested::Auto => AUTO,
        Requested::Fixed(language) => subtag(language),
    }
}

/// The bare language subtag the prompt dictionary is keyed by.
///
/// A `match` with no wildcard, so a ninth [`VoiceLanguage`] is a compile error
/// here rather than a language that quietly transcribes as English — the same
/// discipline `VoiceLanguage::lang` follows in `koru-voice`.
pub fn subtag(language: VoiceLanguage) -> &'static str {
    match language {
        VoiceLanguage::En => "en",
        VoiceLanguage::Es => "es",
        // **Not `pt-BR`.** The dictionary is keyed by language, not by variety,
        // and Plan 0.4's report must not read a Brazilian result out of a
        // prompt that only ever said Portuguese.
        VoiceLanguage::PtBr => "pt",
        VoiceLanguage::De => "de",
        VoiceLanguage::Fr => "fr",
        VoiceLanguage::Ar => "ar",
        // **Not `zh-Hans`.** Same rule; the script is a property of the
        // tokenizer's output, and the benchmark normalises Traditional to
        // Simplified rather than asking the model to promise it.
        VoiceLanguage::ZhHans => "zh",
        VoiceLanguage::Ja => "ja",
    }
}

#[cfg(test)]
mod tests;
