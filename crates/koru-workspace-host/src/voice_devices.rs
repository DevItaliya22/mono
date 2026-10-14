// SPDX-License-Identifier: MPL-2.0

//! **What microphones this machine has — a reading, never a record.**
//!
//! The preference is in [`crate::voice_prefs`] and is a *name*. This is the
//! other half: the list that name is read against, which belongs to the
//! machine rather than to the person and therefore is not stored anywhere at
//! all. It is asked for when a Settings tab is built and when somebody asks
//! for it again, exactly the way
//! [`crate::voice_install::installed_models`] is asked, and for the same
//! reason: a stored list would answer *your headset is here* for a headset
//! that had been unplugged, and nothing would fail until a dictation did.
//!
//! ## Only the worker can enumerate, so this asks the worker
//!
//! `cpal` is linked in exactly one place in this workspace —
//! `apps/koru-voice`'s `capture` module — because that is the process that
//! opens the microphone and the reason PCM never crosses a socket
//! (ADR-249). A window that enumerated for itself would be a window that
//! linked an audio backend, which is the whole arrangement undone for a list
//! of two strings. So the question travels the ordinary voice wire and the
//! answer comes back on it.
//!
//! ## Settings may start an idle worker, and the question blocks
//!
//! Opening the input picker is an explicit request for the live machine list,
//! so it may ask Agent to start the otherwise idle worker. Starting the worker
//! neither loads a model nor opens a microphone; the query arms no dictation.
//! It blocks, bounded, on a socket, so `App` runs it on a **thread**, posts the
//! answer back over the existing voice `mpsc` and wakes the loop, which is the
//! shape `resolve_offer` already has. Passive callers can still use
//! `VoiceClient::devices`, whose no-start contract remains intact.
//!
//! ## A name may be shown and may not be logged
//!
//! Nothing here writes a device name to `tracing`. The split is
//! `koru_voice::input`'s and `apps/koru-voice`'s `capture` module's: a picker
//! has to show what a device is called, and a support log a person attaches to
//! a report may not carry their hardware. Counts are fine, and counts are what
//! the two log lines in this file carry.

use koru_voice::{InputChoice, InputDevice, InputResolution};

/// The last answer, and a counter that moves whenever it changes.
///
/// A process global for exactly the reason the preferences beside it are: two
/// Settings tabs are two views of one machine, and a list held on the view
/// would be re-asked per tab and could disagree between them.
static KNOWN: std::sync::RwLock<Option<Vec<InputDevice>>> = std::sync::RwLock::new(None);

/// Bumped when [`set`] changes the list, so a caller can notice a move without
/// comparing two vectors of strings.
static MOVED: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(0);

/// **Every device the worker last said it could see.**
///
/// Empty until something has asked — which is not the same as *this machine
/// has no microphone*, and is why [`asked`] exists beside it. A surface that
/// confused the two would draw *no microphones found* on a machine whose
/// worker simply is not running.
pub fn known() -> Vec<InputDevice> {
    KNOWN
        .read()
        .unwrap_or_else(|error| error.into_inner())
        .clone()
        .unwrap_or_default()
}

/// Whether an answer has ever arrived. See [`known`] on why the two are
/// different questions.
pub fn asked() -> bool {
    KNOWN
        .read()
        .unwrap_or_else(|error| error.into_inner())
        .is_some()
}

/// Record what the worker answered. `true` when the list actually moved, so a
/// caller can skip a redraw nothing would change.
pub fn set(devices: Vec<InputDevice>) -> bool {
    let mut held = KNOWN.write().unwrap_or_else(|error| error.into_inner());
    if held.as_deref() == Some(devices.as_slice()) {
        return false;
    }
    *held = Some(devices);
    MOVED.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
    true
}

/// A number that changes whenever the list does. For a guard that has to
/// notice, without comparing strings on an event path.
pub fn generation() -> u64 {
    MOVED.load(std::sync::atomic::Ordering::Relaxed)
}

/// **Ask the worker.** Blocking; runs on a worker thread.
///
/// A failure is not an error here and is not reported as one: an installation
/// can run without Agent, and the picker then draws only *System default* and
/// whatever name is remembered — the only two things that are known.
pub fn read(client: &koru_dictation::VoiceClient) -> Vec<InputDevice> {
    // **The same gate the controller applies to a dictation.** A suite above
    // this crate turns `no-engine` on in its `[dev-dependencies]`, and without
    // this line a settings test would dial whatever voice worker is running on
    // the developer's machine -- the `no-reader` lesson, in its third form.
    if koru_dictation::ENGINE_OFF {
        return Vec::new();
    }
    match client.devices_starting_worker() {
        Ok(devices) => {
            // A count, never a name. See the module note.
            tracing::debug!(
                target: "koru::voice",
                devices = devices.len(),
                "the voice worker answered which input devices it can see"
            );
            devices
        }
        Err(error) => {
            tracing::debug!(
                target: "koru::voice",
                code = error.code.as_str(),
                "no voice worker answered which input devices there are"
            );
            Vec::new()
        }
    }
}

/// **What the current preference comes to on this machine**, against the list
/// last read.
///
/// The one place a surface and the press path agree, so a settings row saying
/// *using the system default instead* cannot disagree with what the worker
/// will actually open. The rule itself is [`koru_voice::input::resolve`]'s.
///
/// **Unasked is not missing.** With no answer yet, a remembered name resolves
/// to [`InputResolution::Device`] rather than to a fall-back: it is very
/// probably plugged in, and drawing *that microphone is gone* because nothing
/// has been asked yet would be a sentence about this process rather than about
/// the machine.
pub fn resolution() -> InputResolution {
    let choice = crate::voice_prefs::input();
    if !asked() {
        return match choice {
            InputChoice::SystemDefault => InputResolution::SystemDefault,
            InputChoice::Named { name } => InputResolution::Device(name),
        };
    }
    koru_voice::input::resolve(&choice, &known())
}

#[cfg(test)]
mod tests;
