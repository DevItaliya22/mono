// SPDX-License-Identifier: MPL-2.0

//! **Which microphone, said as a name rather than as a position in a list.**
//!
//! Three types and one function, and between them they are the whole of the
//! choice: what the worker found ([`InputDevice`]), what a person asked for
//! ([`InputChoice`]), and what those two come to together
//! ([`InputResolution`]).
//!
//! ## A name, never an index
//!
//! `cpal` hands back devices in whatever order the host enumerates them, and
//! that order is not stable: unplugging a USB interface renumbers everything
//! after it, a Bluetooth headset appears when it connects and disappears when
//! it sleeps, and a reboot is free to reorder the lot. A stored `2` therefore
//! means *a different microphone* on the next launch, with nothing failing and
//! nothing said — which is the exact shape of defect this codebase writes ADRs
//! about, because the symptom is that dictation quietly listens to the wrong
//! room.
//!
//! So the preference is the device's **name**. It is not unique in theory —
//! two identical USB microphones can present the same string — but it is
//! stable in practice, it is what a person recognises, and when it is wrong it
//! is wrong in a way somebody can see and fix. `resolve` matches it exactly
//! and case-insensitively, and a name that is not there is a *fall-back with a
//! sentence*, never an error and never silence.
//!
//! ## "System default" is a choice, not the absence of one
//!
//! [`InputChoice::SystemDefault`] and a person who has never opened the picker
//! both end up opening the default device, and they are still different
//! answers. Somebody who picked *System default* is saying **follow the OS** —
//! they change headsets and expect Koru to come with them. Somebody who has
//! never chosen has expressed nothing. The two are one variant here because
//! nothing this crate does distinguishes them; what does distinguish them is
//! that the picker draws *System default* as a selectable row, so choosing it
//! is a gesture with a result rather than a way of clearing a field.
//!
//! ## The name crosses the socket, and it may not reach a log
//!
//! `apps/koru-voice`'s `capture` module is explicit that a device's name is a
//! person's hardware — *Steven's AirPods*, the make of a laptop — and that
//! `docs/contributing/operations/LOGGING.md` allows a log a bounded category and not that.
//! A picker cannot be drawn out of categories, so the name is here and it does
//! cross the wire; what does not change is the rule about writing it down.
//! `DeviceCategory` is still what a status report and a log line carry, and
//! nothing in this module is `Display`, so there is no convenient way to put
//! one into a `tracing` field by accident.

use serde::{Deserialize, Serialize};

/// **One capture device the worker can see.**
///
/// The name and whether the host calls it the default — which is all a picker
/// draws. Deliberately not the sample rate, the channel count or the
/// category: those are `koru-voice-worker status`'s, and a settings row that showed
/// them would be a diagnostic wearing a control's clothes.
#[derive(Clone, PartialEq, Eq, Debug, Serialize, Deserialize)]
pub struct InputDevice {
    /// What the operating system calls it. The thing a person recognises, and
    /// the thing [`InputChoice::Named`] stores.
    pub name: String,
    /// Whether this is the device the host would open if nobody chose.
    pub default: bool,
}

impl InputDevice {
    pub fn new(name: impl Into<String>, default: bool) -> InputDevice {
        InputDevice {
            name: name.into(),
            default,
        }
    }
}

/// **What somebody asked to listen with.**
///
/// `Default` is [`InputChoice::SystemDefault`], so a preference file with no
/// row and a person who picked *System default* arrive at the same behaviour —
/// see the module note on why they are still different answers.
#[derive(Clone, PartialEq, Eq, Debug, Default, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum InputChoice {
    /// Follow the operating system, now and whenever it changes.
    #[default]
    SystemDefault,
    /// This device, by name.
    ///
    /// A struct variant rather than a newtype, because the enum is internally
    /// tagged so it can travel beside the rest of the wire — and serde cannot
    /// tag a newtype variant holding a bare string.
    Named { name: String },
}

impl InputChoice {
    /// The choice that names this device.
    pub fn named(name: impl Into<String>) -> InputChoice {
        InputChoice::Named { name: name.into() }
    }
}

/// The token `preferences.conf` carries for [`InputChoice::SystemDefault`].
///
/// A word no device is called, and one that stays the same in every language —
/// **never the label**, which is the mistake PRINCIPLES names and which
/// `voice_prefs::language_token` already avoids for the same reason.
pub const SYSTEM_DEFAULT: &str = "system-default";

impl InputChoice {
    /// The stored token: [`SYSTEM_DEFAULT`], or the device's name verbatim.
    pub fn token(&self) -> &str {
        match self {
            InputChoice::SystemDefault => SYSTEM_DEFAULT,
            InputChoice::Named { name } => name,
        }
    }

    /// The choice a token names.
    ///
    /// **Anything empty or unrecognised is the system default**, which is the
    /// rule `parse_language` already follows: a settings file is not a signed
    /// descriptor, and the answer that is wrong for nobody is *follow the OS*.
    /// A name is otherwise taken as written — this is where a person's own
    /// hardware is round-tripped, so trimming is the only normalisation.
    pub fn parse(token: &str) -> InputChoice {
        let token = token.trim();
        if token.is_empty() || token.eq_ignore_ascii_case(SYSTEM_DEFAULT) {
            return InputChoice::SystemDefault;
        }
        InputChoice::Named {
            name: token.to_owned(),
        }
    }

    /// The device this names, or `None` for the system default.
    pub fn name(&self) -> Option<&str> {
        match self {
            InputChoice::SystemDefault => None,
            InputChoice::Named { name } => Some(name),
        }
    }
}

/// **What a choice and a device list come to together.**
///
/// Three answers, and the third is the whole reason this is a type rather than
/// an `Option<String>`: a remembered device that is not plugged in is neither
/// a failure nor the same thing as never having chosen, and a surface has to
/// be able to say which of those happened.
#[derive(Clone, PartialEq, Eq, Debug)]
pub enum InputResolution {
    /// Follow the operating system — either because that was chosen, or
    /// because nothing ever was.
    SystemDefault,
    /// Open this device. It was asked for and it is present.
    Device(String),
    /// The remembered device is not here. The system default is being opened,
    /// and a surface owes somebody a sentence saying so.
    FellBack {
        /// What was asked for, so the sentence can name it.
        wanted: String,
    },
}

impl InputResolution {
    /// The device to open, or `None` for the host's default.
    pub fn device(&self) -> Option<&str> {
        match self {
            InputResolution::SystemDefault | InputResolution::FellBack { .. } => None,
            InputResolution::Device(name) => Some(name),
        }
    }

    /// The device that was asked for and could not be found.
    pub fn missing(&self) -> Option<&str> {
        match self {
            InputResolution::FellBack { wanted } => Some(wanted),
            _ => None,
        }
    }
}

/// **Read a choice against what is actually plugged in.**
///
/// Exact match first, then case-insensitive, because a host is free to
/// re-case a device string between releases and a capitalisation is not a
/// different microphone. Anything still unmatched is
/// [`InputResolution::FellBack`] — **never an error**: refusing to dictate
/// because a headset is asleep would be a feature that stops working for a
/// reason nobody would connect to the headset.
///
/// A pure function of two values, which is what lets every surface and the
/// worker agree without any of them owning the rule.
pub fn resolve(choice: &InputChoice, devices: &[InputDevice]) -> InputResolution {
    let Some(wanted) = choice.name() else {
        return InputResolution::SystemDefault;
    };
    if let Some(found) = devices
        .iter()
        .find(|d| d.name == wanted)
        .or_else(|| devices.iter().find(|d| d.name.eq_ignore_ascii_case(wanted)))
    {
        return InputResolution::Device(found.name.clone());
    }
    InputResolution::FellBack {
        wanted: wanted.to_owned(),
    }
}

#[cfg(test)]
mod tests;
