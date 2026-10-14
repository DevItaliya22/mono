// SPDX-License-Identifier: MPL-2.0

//! Support logs accept closed categories and normalized events only. Provider
//! payloads and native identifiers never become tracing fields.
use super::*;

#[derive(Clone, Copy)]
struct RouteIdentity {
    provider: &'static str,
    version: &'static str,
}

impl RouteIdentity {
    fn of(route: &AcpRoute) -> Self {
        match route.dialect.registration() {
            Some(registration) => Self {
                provider: registration.kind,
                version: route.dialect.version(),
            },
            None => Self {
                provider: "generic",
                version: "unreviewed",
            },
        }
    }
}

pub(super) fn probe_failure(route: &AcpRoute, error: &ProviderFailure) {
    let route = RouteIdentity::of(route);
    tracing::warn!(target: "koru::acp", event = "acp.probe_failed",
        provider = route.provider, expected_version = route.version,
        stage = ?error.stage, diagnostic = ?error.diagnostic, code = error.code.as_str());
}

#[derive(Default)]
pub(super) struct SessionLog {
    route: Option<RouteIdentity>,
    session_id: Option<SessionId>,
    started: Option<Instant>,
    turn: Option<TurnLog>,
    foreign_sessions: u64,
}

struct TurnLog {
    started: Instant,
    first_output: bool,
    messages: u64,
    tools: u64,
    failed_tools: u64,
    approvals: u64,
}

impl SessionLog {
    pub(super) fn start(&mut self, route: &AcpRoute, delegated: bool, session_id: SessionId) {
        let identity = RouteIdentity::of(route);
        self.route = Some(identity);
        self.session_id = Some(session_id);
        self.started = Some(Instant::now());
        tracing::info!(target: "koru::acp", event = "acp.starting",
            provider = identity.provider, expected_version = identity.version,
            session_id = ?self.session_id, delegated, child_policy = ?route.child_sessions);
        if route.child_sessions == ChildSessionPolicy::Experimental {
            tracing::warn!(target: "koru::acp", event = "acp.experimental_child_control",
                provider = identity.provider, session_id = ?self.session_id,
                native_child_control_verified = false);
        }
    }

    pub(super) fn ready(&self, capabilities: &AgentCapabilities) {
        let Some(route) = self.route else { return };
        tracing::info!(target: "koru::acp", event = "acp.ready",
            provider = route.provider, version = route.version, session_id = ?self.session_id,
            mcp_http = capabilities.delegation.mcp == McpSupport::Supported,
            startup_ms = elapsed(self.started));
    }

    pub(super) fn failure(&self, operation: &'static str, error: &ProviderFailure) {
        let Some(route) = self.route else { return };
        tracing::warn!(target: "koru::acp", event = "acp.failed",
            provider = route.provider, session_id = ?self.session_id, operation,
            stage = ?error.stage, diagnostic = ?error.diagnostic, code = error.code.as_str());
    }

    pub(super) fn foreign_session(&mut self) {
        self.foreign_sessions = self.foreign_sessions.saturating_add(1);
        if self.foreign_sessions == 1 {
            tracing::warn!(target: "koru::acp", event = "acp.foreign_session_signal",
                session_id = ?self.session_id, native_child_spawning_proven = false);
        }
    }

    pub(super) fn cancel_requested(&self, unrepresentable_approval: bool) {
        tracing::info!(target: "koru::acp", event = "acp.cancel_requested",
            session_id = ?self.session_id, unrepresentable_approval);
    }

    pub(super) fn cancel_deadline(&self) {
        tracing::warn!(target: "koru::acp", event = "acp.cancel_deadline",
            session_id = ?self.session_id);
    }

    pub(super) fn observe(&mut self, event: &AgentEvent, counters: ProviderDiagnostics) {
        match event {
            AgentEvent::TurnStarted { .. } => {
                self.turn = Some(TurnLog {
                    started: Instant::now(),
                    first_output: false,
                    messages: 0,
                    tools: 0,
                    failed_tools: 0,
                    approvals: 0,
                });
                tracing::info!(target: "koru::acp", event = "acp.turn_started",
                    session_id = ?self.session_id);
            }
            AgentEvent::MessageDelta { .. } => {
                self.first_output();
                if let Some(turn) = &mut self.turn {
                    turn.messages = turn.messages.saturating_add(1);
                }
            }
            AgentEvent::ToolStarted { .. } => {
                self.first_output();
                if let Some(turn) = &mut self.turn {
                    turn.tools = turn.tools.saturating_add(1);
                }
                tracing::debug!(target: "koru::acp", event = "acp.tool_started",
                    session_id = ?self.session_id);
            }
            AgentEvent::ToolCompleted {
                succeeded, tool, ..
            } => {
                if let Some(turn) = &mut self.turn {
                    turn.failed_tools = turn.failed_tools.saturating_add(u64::from(!succeeded));
                }
                tracing::debug!(target: "koru::acp", event = "acp.tool_completed",
                    session_id = ?self.session_id, succeeded, kind = ?tool);
            }
            AgentEvent::ApprovalRequired { choices, .. } => {
                if let Some(turn) = &mut self.turn {
                    turn.approvals = turn.approvals.saturating_add(1);
                }
                tracing::debug!(target: "koru::acp", event = "acp.approval_required",
                    session_id = ?self.session_id, choices = choices.len());
            }
            AgentEvent::ApprovalResolved { decision, .. } => {
                tracing::debug!(target: "koru::acp", event = "acp.approval_resolved",
                    session_id = ?self.session_id, decision = ?decision);
            }
            AgentEvent::ApprovalExpired { .. } => {
                tracing::debug!(target: "koru::acp", event = "acp.approval_expired",
                    session_id = ?self.session_id);
            }
            AgentEvent::TurnCompleted { .. } => self.finish("completed", None, counters),
            AgentEvent::TurnCancelled { .. } => self.finish("cancelled", None, counters),
            AgentEvent::TurnFailed { error, .. } => {
                self.finish("failed", Some(error.code.as_str()), counters)
            }
            _ => {}
        }
    }

    fn first_output(&mut self) {
        if let Some(turn) = &mut self.turn
            && !turn.first_output
        {
            turn.first_output = true;
            tracing::info!(target: "koru::acp", event = "acp.first_output",
                session_id = ?self.session_id, turn_elapsed_ms = elapsed(Some(turn.started)));
        }
    }

    fn finish(
        &mut self,
        outcome: &'static str,
        error_code: Option<&str>,
        counters: ProviderDiagnostics,
    ) {
        let Some(turn) = self.turn.take() else { return };
        tracing::info!(target: "koru::acp", event = "acp.turn_finished",
            session_id = ?self.session_id, outcome, error_code = error_code.unwrap_or("none"), turn_elapsed_ms = elapsed(Some(turn.started)),
            message_chunks = turn.messages, tools = turn.tools, failed_tools = turn.failed_tools,
            approvals = turn.approvals, session_unknown_events = counters.unknown_events,
            session_foreign_signals = self.foreign_sessions,
            session_queue_high_water = counters.maximum_queue_depth);
    }

    pub(super) fn shutdown(
        &mut self,
        result: &Result<(), ProviderFailure>,
        counters: ProviderDiagnostics,
    ) {
        if self.route.is_none() {
            return;
        }
        self.finish("shutdown", None, counters);
        if let Err(error) = result {
            self.failure("shutdown", error);
        }
        tracing::info!(target: "koru::acp", event = "acp.shutdown",
            session_id = ?self.session_id, succeeded = result.is_ok(),
            session_elapsed_ms = elapsed(self.started), frames = counters.frames_read,
            unknown_events = counters.unknown_events, malformed_frames = counters.malformed_frames,
            foreign_session_signals = self.foreign_sessions);
    }
}

fn elapsed(started: Option<Instant>) -> u64 {
    started.map_or(0, |started| {
        started.elapsed().as_millis().min(u64::MAX as u128) as u64
    })
}
