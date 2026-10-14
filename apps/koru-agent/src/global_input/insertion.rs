// SPDX-License-Identifier: MPL-2.0

/// The result of comparing the exact captured accessibility element with the
/// element focused now. Controls inside one HWND remain distinct.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum TargetState {
    Editable,
    Protected,
    Changed,
    Destroyed,
    /// Accessibility could not prove identity or password status. Unknown is
    /// unsafe and therefore closed, never treated as an ordinary field.
    Unavailable,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum DirectInsertion {
    Complete,
    /// No input event was accepted. Windows refuses visibly and leaves the
    /// clipboard untouched because bounded paste completion cannot be proven.
    Rejected,
    /// Some input events were accepted. Falling back would duplicate text.
    Partial,
    /// The platform could no longer prove that insertion was safe.
    TargetUnavailable,
}

/// Stable, content-free reasons a surface can localize and show.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Refusal {
    NoTarget,
    ProtectedTarget,
    TargetChanged,
    TargetDestroyed,
    TargetUnavailable,
    ClipboardCompletionUnprovable,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Commit {
    Inserted,
    /// Some direct input events were accepted. No fallback ran, but callers
    /// must not retry because the target may contain a prefix of the text.
    DirectPartial,
    Refused(Refusal),
}

/// Content-free geometry for placing status beside an attempted target.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct TargetDisplay {
    pub work_area: (i32, i32, i32, i32),
    pub dpi: u32,
}

/// Platform mechanism only. Implementations must not derive target identity
/// from a window title, process, path, or timestamp.
pub trait InsertBackend {
    type Target: Clone;
    type RestoreHint: Clone;

    /// Capture only an opaque top-level application identity. This is allowed
    /// before Voice consent because it performs no accessibility inspection
    /// and retains no control, title, content, or executable path.
    fn capture_restore_hint(&mut self) -> Result<Self::RestoreHint, Refusal>;
    /// Restore and prove the exact top-level application before a fresh,
    /// policy-gated accessibility capture.
    fn restore_hint(&mut self, hint: &Self::RestoreHint) -> Result<(), Refusal>;
    fn capture_target(&mut self) -> Result<Self::Target, Refusal>;
    /// Capture an accessibility target and prove it still belongs to the
    /// restored top-level application. This closes the focus race between
    /// restoring a hint and acquiring the exact control.
    fn capture_target_for_hint(
        &mut self,
        hint: &Self::RestoreHint,
    ) -> Result<Self::Target, Refusal>;
    fn target_state(&mut self, target: &Self::Target) -> TargetState;
    /// Ask the platform to return focus, then prove the exact captured
    /// accessibility element. The default is useful for deterministic fakes.
    fn restore_target(&mut self, target: &Self::Target) -> TargetState {
        self.target_state(target)
    }
    /// Capture only work-area geometry for the current top-level target root.
    /// This performs no accessibility inspection and carries no identity.
    fn current_display(&mut self) -> Option<TargetDisplay> {
        None
    }
    /// Derive current geometry from the already captured/proven target root.
    fn target_display(&mut self, _target: &Self::Target) -> Option<TargetDisplay> {
        None
    }
    fn insert_unicode(&mut self, target: &Self::Target, text: &str) -> DirectInsertion;
}

/// An opaque, content-free application restore token.
pub struct CapturedRestoreHint<T> {
    hint: T,
}

impl<T> std::fmt::Debug for CapturedRestoreHint<T> {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str("CapturedRestoreHint([REDACTED])")
    }
}

pub fn capture_restore_hint<B: InsertBackend>(
    backend: &mut B,
) -> Result<CapturedRestoreHint<B::RestoreHint>, Refusal> {
    backend
        .capture_restore_hint()
        .map(|hint| CapturedRestoreHint { hint })
}

pub fn restore_hint<B: InsertBackend>(
    backend: &mut B,
    captured: &CapturedRestoreHint<B::RestoreHint>,
) -> Result<(), Refusal> {
    backend.restore_hint(&captured.hint)
}

pub fn capture_for_hint<B: InsertBackend>(
    backend: &mut B,
    hint: &CapturedRestoreHint<B::RestoreHint>,
) -> Result<CapturedTarget<B::Target>, Refusal> {
    let target = backend.capture_target_for_hint(&hint.hint)?;
    match backend.target_state(&target) {
        TargetState::Editable => Ok(CapturedTarget { target }),
        state => Err(refusal_for(state)),
    }
}

pub fn restore<B: InsertBackend>(
    backend: &mut B,
    captured: &CapturedTarget<B::Target>,
) -> Result<(), Refusal> {
    match backend.restore_target(&captured.target) {
        TargetState::Editable => Ok(()),
        state => Err(refusal_for(state)),
    }
}

pub fn target_display<B: InsertBackend>(
    backend: &mut B,
    captured: &CapturedTarget<B::Target>,
) -> Option<TargetDisplay> {
    backend.target_display(&captured.target)
}

/// A target is deliberately opaque in diagnostics. The platform token is
/// private, and Debug never walks into it.
pub struct CapturedTarget<T> {
    target: T,
}

impl<T> std::fmt::Debug for CapturedTarget<T> {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str("CapturedTarget([REDACTED])")
    }
}

pub fn capture<B: InsertBackend>(backend: &mut B) -> Result<CapturedTarget<B::Target>, Refusal> {
    let target = backend.capture_target()?;
    match backend.target_state(&target) {
        TargetState::Editable => Ok(CapturedTarget { target }),
        state => Err(refusal_for(state)),
    }
}

pub fn commit<B: InsertBackend>(
    backend: &mut B,
    captured: &CapturedTarget<B::Target>,
    text: &str,
) -> Commit {
    if let Err(refusal) = verify(backend, &captured.target) {
        return Commit::Refused(refusal);
    }

    match backend.insert_unicode(&captured.target, text) {
        DirectInsertion::Complete => Commit::Inserted,
        DirectInsertion::Partial => Commit::DirectPartial,
        DirectInsertion::TargetUnavailable => Commit::Refused(Refusal::TargetUnavailable),
        DirectInsertion::Rejected => Commit::Refused(Refusal::ClipboardCompletionUnprovable),
    }
}

fn verify<B: InsertBackend>(backend: &mut B, target: &B::Target) -> Result<(), Refusal> {
    match backend.target_state(target) {
        TargetState::Editable => Ok(()),
        state => Err(refusal_for(state)),
    }
}

fn refusal_for(state: TargetState) -> Refusal {
    match state {
        TargetState::Editable => unreachable!("an editable target is not a refusal"),
        TargetState::Protected => Refusal::ProtectedTarget,
        TargetState::Changed => Refusal::TargetChanged,
        TargetState::Destroyed => Refusal::TargetDestroyed,
        TargetState::Unavailable => Refusal::TargetUnavailable,
    }
}
