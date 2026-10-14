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
    /// No input event was accepted. The caller must not retry through another mechanism.
    Rejected,
    /// Some input events were accepted. Falling back would duplicate text.
    Partial,
    /// The platform could no longer prove that insertion was safe.
    TargetUnavailable,
}

/// Content-free reasons the native mechanism could not prove a target.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Refusal {
    NoTarget,
    ProtectedTarget,
    TargetChanged,
    TargetDestroyed,
    TargetUnavailable,
}

/// Content-free geometry for placing status beside an attempted target.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct TargetDisplay {
    pub work_area: (i32, i32, i32, i32),
    pub dpi: u32,
}
