// SPDX-License-Identifier: MPL-2.0

//! Private consistency check for the exact Grok inventory carrier. The process
//! policy and installed-binary refusal proof establish child exclusion; this
//! rejects a session whose complete inventory contradicts that policy.
use super::*;

impl AcpControl {
    pub(super) fn grok_inventory_update(
        &mut self,
        params: &Value,
    ) -> Result<bool, ProviderFailure> {
        if params
            .pointer("/update/sessionUpdate")
            .and_then(Value::as_str)
            != Some("available_commands_update")
        {
            return Ok(false);
        }
        let session = params["sessionId"]
            .as_str()
            .filter(|id| valid_native_id(id))
            .ok_or_else(handshake_rejected)?;
        if self
            .native_session
            .as_deref()
            .is_some_and(|current| current != session)
        {
            self.mark_unknown();
            return Ok(true);
        }
        // Missing, malformed or empty inventories do not prove the reviewed
        // carrier remains complete. Native names are inspected, never retained.
        let valid = params
            .pointer("/update/_meta/tools")
            .and_then(Value::as_array)
            .is_some_and(|tools| {
                !tools.is_empty()
                    && tools.iter().all(|tool| {
                        tool.as_str().is_some_and(|name| {
                            valid_native_id(name)
                                && !matches!(name, "spawn_subagent" | "task" | "Task" | "Agent")
                        })
                    })
            });
        if !valid {
            let error = handshake_rejected();
            self.fail_transport(error.clone());
            let _ = self.shutdown();
            return Err(error);
        }
        // A single bounded private identity is sufficient: only one native
        // session is created by this owner, and startup binds it below.
        self.grok_inventory = Some(session.to_owned());
        Ok(true)
    }

    pub(super) fn await_grok_inventory(&mut self) -> Result<(), ProviderFailure> {
        let deadline = Instant::now() + self.config.request_timeout;
        while self.grok_inventory.as_deref() != self.native_session.as_deref() {
            let remaining = deadline.saturating_duration_since(Instant::now());
            if remaining.is_zero() {
                return self.grok_inventory_timeout();
            }
            match self.transport()?.receive(remaining)? {
                TransportReceive::Frame(frame) => {
                    self.diagnostics.frames_read = self.diagnostics.frames_read.saturating_add(1);
                    self.handle_frame(frame)?;
                }
                TransportReceive::Timeout => return self.grok_inventory_timeout(),
                TransportReceive::Woken => {}
            }
        }
        Ok(())
    }

    fn grok_inventory_timeout(&mut self) -> Result<(), ProviderFailure> {
        let error = failure(
            ErrorCode::ProviderUnavailable,
            ProviderStage::Initialize,
            ProviderDiagnostic::Timeout,
        );
        self.fail_transport(error.clone());
        let _ = self.shutdown();
        Err(error)
    }
}
