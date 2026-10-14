// SPDX-License-Identifier: MPL-2.0

//! The linked-devices model: what Agent last said about this device, its
//! linked peers and the relay, plus the history page one peer has open and
//! the presence-refresh deadline that keeps the list current while it shows.
//!
//! It holds no pixels. The window drains Agent's single-consumer snapshot —
//! whether or not anything is showing it — into one of these for the names
//! and the count, and hands every snapshot and history page on to the Manage
//! tab's own, which is the one the *Linked devices* page draws from and the
//! one whose history page and presence deadline are ever used.

use std::time::{Duration, Instant};

use koru_agent_wire::{DeviceStateEventView, LinkedDeviceView, RelayView};

/// How often the list is re-asked for while someone is looking at it, so a
/// peer going online or offline shows without a click.
pub(crate) const PRESENCE_REFRESH: Duration = Duration::from_secs(1);

#[derive(Clone, Default)]
pub(crate) struct LinkedDevices {
    /// This device itself — the one row a rename gesture lands on.
    this_device: Option<LinkedDeviceView>,
    linked: Vec<LinkedDeviceView>,
    relay: Option<RelayView>,
    /// The peer whose state-change history is open, by device id.
    history_device: Option<String>,
    /// The cursor for the next older page, when Agent said there is one.
    history_next: Option<i64>,
    history_loading: bool,
    presence_refresh_at: Option<Instant>,
    presence_refresh_due: bool,
}

impl LinkedDevices {
    pub(crate) fn this_device(&self) -> Option<&LinkedDeviceView> {
        self.this_device.as_ref()
    }

    pub(crate) fn devices(&self) -> &[LinkedDeviceView] {
        &self.linked
    }

    pub(crate) fn relay(&self) -> Option<&RelayView> {
        self.relay.as_ref()
    }

    /// The peer whose history is open, while it is still linked.
    pub(crate) fn history_device(&self) -> Option<&LinkedDeviceView> {
        self.history_device
            .as_ref()
            .and_then(|id| self.linked.iter().find(|device| &device.device_id == id))
    }

    pub(crate) fn history_open(&self) -> bool {
        self.history_device.is_some()
    }

    pub(crate) fn history_loading(&self) -> bool {
        self.history_loading
    }

    /// Whether Agent said an older page of the open history exists and it
    /// is not already being fetched.
    pub(crate) fn has_older_history(&self) -> bool {
        self.history_next.is_some() && !self.history_loading
    }

    /// Adopt Agent's whole snapshot. History already fetched for a peer
    /// survives the refresh; a peer that is gone closes its history page.
    pub(crate) fn set_linked(
        &mut self,
        this_device: Option<LinkedDeviceView>,
        linked: Vec<LinkedDeviceView>,
        relay: Option<RelayView>,
    ) {
        self.this_device = this_device;
        let mut linked = linked;
        for device in &mut linked {
            if let Some(previous) = self
                .linked
                .iter()
                .find(|previous| previous.device_id == device.device_id)
            {
                device.history = previous.history.clone();
            }
        }
        self.linked = linked;
        self.relay = relay;
        if self
            .history_device
            .as_ref()
            .is_some_and(|id| !self.linked.iter().any(|device| &device.device_id == id))
        {
            self.close_history();
        }
    }

    /// Append one page of a peer's history. A page for a peer that is not
    /// the open one is stale and dropped.
    pub(crate) fn set_history(
        &mut self,
        device_id: &str,
        events: Vec<DeviceStateEventView>,
        next_before: Option<i64>,
    ) {
        if self.history_device.as_deref() != Some(device_id) {
            return;
        }
        if let Some(device) = self
            .linked
            .iter_mut()
            .find(|device| device.device_id == device_id)
        {
            for event in events {
                if !device
                    .history
                    .iter()
                    .any(|existing| existing.event_id == event.event_id)
                {
                    device.history.push(event);
                }
            }
        }
        self.history_next = next_before;
        self.history_loading = false;
    }

    /// Open the history of the peer at `index`, starting from a clean page.
    /// Returns the device id whose first page must be asked for.
    pub(crate) fn open_history(&mut self, index: usize) -> Option<String> {
        let device_id = self
            .linked
            .get(index)
            .map(|device| device.device_id.clone());
        self.history_device = device_id.clone();
        self.history_next = None;
        self.history_loading = device_id.is_some();
        if let Some(device) = self.linked.get_mut(index) {
            device.history.clear();
        }
        device_id
    }

    pub(crate) fn close_history(&mut self) {
        self.history_device = None;
        self.history_next = None;
        self.history_loading = false;
    }

    /// The next older page to ask for, once: the cursor is consumed and the
    /// page is marked loading until it arrives.
    pub(crate) fn take_older_history(&mut self) -> Option<(String, i64)> {
        if self.history_loading {
            return None;
        }
        let device_id = self.history_device.clone()?;
        let before = self.history_next.take()?;
        self.history_loading = true;
        Some((device_id, before))
    }

    /// Flip the relay switch optimistically; the next snapshot confirms it.
    /// Returns the new setting, or `None` when Agent reported no relay.
    pub(crate) fn toggle_relay(&mut self) -> Option<bool> {
        let relay = self.relay.as_mut()?;
        relay.enabled = !relay.enabled;
        relay.state = if relay.enabled { "connecting" } else { "off" }.into();
        if !relay.enabled {
            relay.host = None;
        }
        Some(relay.enabled)
    }

    /// Re-ask for presence on a deadline from `now` while the list shows.
    pub(crate) fn watch_presence(&mut self, now: Instant) {
        self.presence_refresh_at = Some(now + PRESENCE_REFRESH);
    }

    pub(crate) fn stop_presence(&mut self) {
        self.presence_refresh_at = None;
        self.presence_refresh_due = false;
    }

    pub(crate) fn presence_deadline(&self) -> Option<Instant> {
        self.presence_refresh_at
    }

    /// Arm the presence refresh when its deadline has passed. The deadline
    /// moves forward; the refresh is taken once by `take_presence_refresh`.
    pub(crate) fn maintain(&mut self, now: Instant) {
        if self
            .presence_refresh_at
            .is_some_and(|deadline| now >= deadline)
        {
            self.presence_refresh_at = Some(now + PRESENCE_REFRESH);
            self.presence_refresh_due = true;
        }
    }

    pub(crate) fn take_presence_refresh(&mut self) -> bool {
        std::mem::take(&mut self.presence_refresh_due)
    }

    /// A fresh viewing session: no history page open and no presence
    /// deadline. The devices themselves are kept — they are still true.
    pub(crate) fn reset_view(&mut self) {
        self.close_history();
        self.stop_presence();
    }
}

#[cfg(test)]
mod tests;
