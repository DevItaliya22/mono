// SPDX-License-Identifier: MPL-2.0

//! Revocation is independent of the thread blocked in a UIA provider.
use std::sync::{Arc, Mutex};

/// A permit for one consequence. Never hold its lock across a provider call.
/// Revocation waits only for an already-started, finite native input batch.
#[derive(Clone, Debug)]
pub struct DeliveryPermit(Arc<Mutex<bool>>);

impl Default for DeliveryPermit {
    fn default() -> Self {
        Self(Arc::new(Mutex::new(true)))
    }
}

impl DeliveryPermit {
    pub fn revoke(&self) {
        if let Ok(mut live) = self.0.lock() {
            *live = false;
        }
    }
    pub fn is_live(&self) -> bool {
        self.0.lock().is_ok_and(|live| *live)
    }
    pub fn consequence<T>(&self, deliver: impl FnOnce() -> T) -> Option<T> {
        let live = self.0.lock().ok()?;
        if !*live {
            return None;
        }
        Some(deliver())
    }
}

#[cfg(test)]
mod tests;
