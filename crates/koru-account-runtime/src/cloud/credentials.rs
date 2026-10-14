// SPDX-License-Identifier: MPL-2.0

//! Read accepted account changes during a blocking dataset session. The worker
//! remains the sole command consumer; this view never removes or applies them.
use super::*;

pub struct SessionAccess {
    pub(super) authority: EnrollmentAuthorization,
    pub(super) desired: Option<Arc<Mutex<Desired>>>,
    pub(super) cancel: Cancel,
    pub(super) foreground: Option<Arc<AtomicBool>>,
}

#[cfg(test)]
mod tests;

impl SessionAccess {
    pub(super) fn current(&self) -> Option<EnrollmentAuthorization> {
        if self.cancel.cancelled()
            || self
                .foreground
                .as_ref()
                .is_some_and(|active| !active.load(Ordering::Acquire))
        {
            return None;
        }
        let mut current = self.authority.clone();
        if let Some(desired) = &self.desired {
            let desired = desired.lock().unwrap_or_else(|error| error.into_inner());
            if desired.stop {
                return None;
            }
            for command in &desired.commands {
                match command {
                    Command::Refresh(previous, next)
                        if same_authority(&current, previous) && same_authority(previous, next) =>
                    {
                        current = next.clone();
                    }
                    Command::Activate(Some(next), _) if same_authority(&current, next) => {
                        current = next.clone();
                    }
                    // Even switching away and back invalidates this session.
                    Command::Activate(..) | Command::Cancel => return None,
                    _ => {}
                }
            }
        }
        Some(current)
    }

    /// The latest accepted bearer for this same account/session. None means
    /// stop this dataset: cancellation, sign-out or account replacement.
    pub fn access_token(&self) -> Option<koru_account_wire::AccessToken> {
        self.current().map(|authority| authority.access)
    }
}
