// SPDX-License-Identifier: MPL-2.0

//! Bounded local admission tracking; it never manufactures an account result.
use koru_agent_wire::Response;
use koru_workspace_host::account_ui::{CommandAdmission, CommandFailure, CommandSubmissionId};
use std::collections::{HashMap, VecDeque};
use std::time::{Duration, Instant};

const CAPACITY: usize = 32;
pub(super) const DEADLINE: Duration = Duration::from_secs(10);

struct Pending {
    submission: CommandSubmissionId,
    connection: u64,
    deadline: Instant,
}

#[derive(Default)]
pub(super) struct Admissions {
    pending: HashMap<u64, Pending>,
    failures: VecDeque<CommandFailure>,
    admitted: VecDeque<CommandAdmission>,
}

impl Admissions {
    pub fn insert(
        &mut self,
        request: u64,
        submission: CommandSubmissionId,
        connection: u64,
        now: Instant,
    ) -> bool {
        if self.pending.len() >= CAPACITY {
            self.fail(submission, false);
            return false;
        }
        self.pending.insert(
            request,
            Pending {
                submission,
                connection,
                deadline: now + DEADLINE,
            },
        );
        true
    }

    pub fn fail(&mut self, submission: CommandSubmissionId, uncertain: bool) {
        if self.failures.len() == CAPACITY {
            self.failures.pop_front();
        }
        self.failures.push_back(CommandFailure {
            submission,
            uncertain,
        });
    }

    pub fn failed_send(&mut self, request: u64) {
        if let Some(pending) = self.pending.remove(&request) {
            self.fail(pending.submission, false);
        }
    }

    pub fn response(&mut self, request: u64, connection: u64, response: &Response) -> bool {
        if !self
            .pending
            .get(&request)
            .is_some_and(|pending| pending.connection == connection)
        {
            return false;
        }
        let pending = self.pending.remove(&request).expect("matched request");
        match response {
            Response::AccountQueued {
                owner_scope,
                generation,
            } => {
                if self.admitted.len() == CAPACITY {
                    self.admitted.pop_front();
                }
                self.admitted.push_back(CommandAdmission {
                    submission: pending.submission,
                    owner_scope: *owner_scope,
                    generation: *generation,
                });
            }
            Response::Failed { .. } => self.fail(pending.submission, false),
            _ => self.fail(pending.submission, true),
        }
        true
    }

    pub fn expire(&mut self, now: Instant) -> bool {
        let expired = self
            .pending
            .iter()
            .filter_map(|(&id, pending)| (pending.deadline <= now).then_some(id))
            .collect::<Vec<_>>();
        let changed = !expired.is_empty();
        for id in expired {
            let pending = self.pending.remove(&id).expect("collected request");
            self.fail(pending.submission, true);
        }
        changed
    }

    pub fn retire(&mut self, connection: u64) -> bool {
        let retired = self
            .pending
            .iter()
            .filter_map(|(&id, pending)| (pending.connection == connection).then_some(id))
            .collect::<Vec<_>>();
        let changed = !retired.is_empty();
        for id in retired {
            let pending = self.pending.remove(&id).expect("collected request");
            self.fail(pending.submission, true);
        }
        changed
    }

    pub fn replace(&mut self, connection: u64) -> bool {
        let previous = self
            .pending
            .values()
            .map(|pending| pending.connection)
            .filter(|&epoch| epoch != connection)
            .collect::<Vec<_>>();
        let mut changed = false;
        for epoch in previous {
            changed |= self.retire(epoch);
        }
        changed
    }

    pub fn take_failure(&mut self) -> Option<CommandFailure> {
        self.failures.pop_front()
    }
    pub fn take_admission(&mut self) -> Option<CommandAdmission> {
        self.admitted.pop_front()
    }
    pub fn deadline(&self) -> Option<Instant> {
        self.pending.values().map(|pending| pending.deadline).min()
    }
}

#[cfg(test)]
mod tests;
