// SPDX-License-Identifier: MPL-2.0

use koru_events::{
    TerminalAttachmentDelivered, TerminalDeliveryMode, TerminalDeliveryOutcome, TerminalInputKind,
    TerminalInputSubmitted, TerminalMode, TerminalProfileKind, TerminalProviderKind,
    TerminalSessionEnded, TerminalSessionOutcome, TerminalSessionReattached,
    TerminalSessionStarted, TerminalSurface, TerminalSurfaceOpened, bucket,
};
use koru_session_protocol::{
    AttachmentId, ProfileKind, ProviderKind, SessionId, SessionMode as ProtocolSessionMode,
};

pub fn metric_mode(mode: &ProtocolSessionMode) -> TerminalMode {
    match mode {
        ProtocolSessionMode::RawPty => TerminalMode::Raw,
        ProtocolSessionMode::Managed { .. } => TerminalMode::Managed,
    }
}

pub fn metric_profile_kind(kind: ProfileKind) -> TerminalProfileKind {
    match kind {
        ProfileKind::Shell => TerminalProfileKind::Shell,
        ProfileKind::Agent => TerminalProfileKind::Agent,
        ProfileKind::Custom => TerminalProfileKind::Custom,
    }
}

/// Collapse the protocol's additive provider token into the metrics registry's
/// reviewed vocabulary. Provider-native IDs never reach this boundary.
pub fn metric_provider_kind(provider: Option<&ProviderKind>) -> TerminalProviderKind {
    match provider.map(ProviderKind::as_str) {
        None => TerminalProviderKind::None,
        Some("claude") => TerminalProviderKind::Claude,
        Some("opencode") => TerminalProviderKind::Opencode,
        Some("codex") => TerminalProviderKind::Codex,
        Some("grok") => TerminalProviderKind::Grok,
        Some("kimi") => TerminalProviderKind::Kimi,
        Some(_) => TerminalProviderKind::Other,
    }
}

/// Explicit gate for session-specific trace events. The disabled path returns
/// before formatting IDs or constructing content strings; `tracing` still
/// applies its ordinary target/level filter when enabled.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct SessionTrace {
    enabled: bool,
}

impl SessionTrace {
    pub fn from_env() -> Self {
        Self::new(koru_env::switch(koru_env::name::KORU_SESSION_DEBUG))
    }

    pub const fn new(enabled: bool) -> Self {
        Self { enabled }
    }

    pub const fn enabled(self) -> bool {
        self.enabled
    }
}

/// One transition owner calls `commit`: the trace fact and typed Koru Event
/// are produced together, but from independent allowlists.
pub struct SurfaceOpenedFact {
    pub surface: TerminalSurface,
}

impl SurfaceOpenedFact {
    pub fn commit(self, trace: SessionTrace) -> TerminalSurfaceOpened {
        if trace.enabled {
            tracing::event!(
                target: "koru_session",
                tracing::Level::INFO,
                event = "terminal.surface.opened",
                surface = self.surface.token(),
            );
        }
        TerminalSurfaceOpened {
            surface: self.surface,
        }
    }
}

pub struct SessionStartedFact {
    pub session_id: SessionId,
    pub mode: TerminalMode,
    pub profile_kind: TerminalProfileKind,
    pub provider_kind: TerminalProviderKind,
    pub elapsed_ms: u64,
}

impl SessionStartedFact {
    pub fn commit(self, trace: SessionTrace) -> TerminalSessionStarted {
        if trace.enabled {
            tracing::event!(
                target: "koru_session",
                tracing::Level::INFO,
                event = "terminal.session.started",
                session_id = %self.session_id,
                mode = self.mode.token(),
                profile_kind = self.profile_kind.token(),
                provider_kind = self.provider_kind.token(),
                elapsed_ms = self.elapsed_ms,
            );
        }
        TerminalSessionStarted {
            mode: self.mode,
            profile_kind: self.profile_kind,
            provider_kind: self.provider_kind,
        }
    }
}

pub struct SessionReattachedFact {
    pub session_id: SessionId,
}

impl SessionReattachedFact {
    pub fn commit(self, trace: SessionTrace) -> TerminalSessionReattached {
        if trace.enabled {
            tracing::event!(
                target: "koru_session",
                tracing::Level::INFO,
                event = "terminal.session.reattached",
                session_id = %self.session_id,
                attached = true,
                reason = "controller_reattached",
            );
        }
        TerminalSessionReattached
    }
}

pub struct SessionEndedFact {
    pub session_id: SessionId,
    pub outcome: TerminalSessionOutcome,
    pub elapsed_ms: u64,
}

impl SessionEndedFact {
    pub fn commit(self, trace: SessionTrace) -> TerminalSessionEnded {
        if trace.enabled {
            tracing::event!(
                target: "koru_session",
                tracing::Level::INFO,
                event = "terminal.session.ended",
                session_id = %self.session_id,
                exit_kind = self.outcome.token(),
                elapsed_ms = self.elapsed_ms,
            );
        }
        TerminalSessionEnded {
            outcome: self.outcome,
            duration_minutes_bucket: bucket(self.elapsed_ms / 60_000),
        }
    }
}

pub struct InputSubmittedFact {
    pub session_id: SessionId,
    pub input_kind: TerminalInputKind,
}

impl InputSubmittedFact {
    pub fn commit(self, trace: SessionTrace) -> TerminalInputSubmitted {
        if trace.enabled {
            tracing::event!(
                target: "koru_session",
                tracing::Level::DEBUG,
                event = "terminal.input.submitted",
                session_id = %self.session_id,
                input_kind = self.input_kind.token(),
            );
        }
        TerminalInputSubmitted {
            input_kind: self.input_kind,
        }
    }
}

pub struct AttachmentDeliveredFact {
    pub session_id: SessionId,
    pub attachment_id: AttachmentId,
    pub delivery_mode: TerminalDeliveryMode,
    pub outcome: TerminalDeliveryOutcome,
}

impl AttachmentDeliveredFact {
    pub fn commit(self, trace: SessionTrace) -> TerminalAttachmentDelivered {
        if trace.enabled {
            tracing::event!(
                target: "koru_session",
                tracing::Level::INFO,
                event = "terminal.attachment.delivered",
                session_id = %self.session_id,
                attachment_id = %self.attachment_id,
                delivery_mode = self.delivery_mode.token(),
                state = self.outcome.token(),
            );
        }
        TerminalAttachmentDelivered {
            delivery_mode: self.delivery_mode,
            outcome: self.outcome,
        }
    }
}
