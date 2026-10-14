// SPDX-License-Identifier: MPL-2.0

use super::*;

fn device(id: &str) -> LinkedDeviceView {
    LinkedDeviceView {
        device_id: id.into(),
        display_name: id.into(),
        linked_at_millis: 0,
        connected: true,
        last_seen_millis: None,
        introduced_by: None,
        build_label: None,
        build_drift: None,
        build_severity: None,
        update_needed_on: None,
        connection: None,
        last_sync_millis: None,
        history: Vec::new(),
    }
}

fn event(id: i64) -> DeviceStateEventView {
    DeviceStateEventView {
        event_id: Some(id),
        at_millis: id,
        kind: "document_updated".into(),
        profile_id: None,
        workspace_id: None,
        document_id: None,
        document_name: None,
        blob_hash: None,
        detail: None,
        items: None,
    }
}

fn relay(enabled: bool) -> RelayView {
    RelayView {
        enabled,
        state: "connected".into(),
        host: Some("dev1.relay.koru.do".into()),
    }
}

#[test]
fn a_snapshot_replaces_the_devices_and_keeps_fetched_history() {
    let mut model = LinkedDevices::default();
    model.set_linked(
        Some(device("this")),
        vec![device("phone"), device("laptop")],
        None,
    );
    assert_eq!(model.open_history(0).as_deref(), Some("phone"));
    model.set_history("phone", vec![event(2), event(1)], None);

    // The next snapshot carries no history; what was fetched survives it.
    model.set_linked(
        Some(device("this")),
        vec![device("phone")],
        Some(relay(true)),
    );
    assert_eq!(model.this_device().unwrap().device_id, "this");
    assert_eq!(model.devices().len(), 1);
    assert_eq!(model.devices()[0].history.len(), 2);
    assert!(model.relay().unwrap().enabled);
    assert_eq!(model.history_device().unwrap().device_id, "phone");
}

#[test]
fn a_peer_that_disappears_closes_its_history_page() {
    let mut model = LinkedDevices::default();
    model.set_linked(None, vec![device("phone")], None);
    model.open_history(0);
    assert!(model.history_loading());
    model.set_linked(None, Vec::new(), None);
    assert!(!model.history_open());
    assert!(!model.history_loading());
    assert!(model.history_device().is_none());
}

#[test]
fn history_pages_append_once_and_ignore_other_devices() {
    let mut model = LinkedDevices::default();
    model.set_linked(None, vec![device("phone"), device("tablet")], None);
    model.open_history(0);
    // A page for a device that is not open is stale.
    model.set_history("tablet", vec![event(9)], Some(1));
    assert!(model.devices()[1].history.is_empty());
    assert!(model.history_loading());

    model.set_history("phone", vec![event(3), event(2)], Some(2));
    model.set_history("phone", vec![event(2), event(1)], None);
    let ids: Vec<_> = model.devices()[0]
        .history
        .iter()
        .map(|event| event.event_id)
        .collect();
    assert_eq!(ids, vec![Some(3), Some(2), Some(1)]);
    assert!(!model.history_loading());
}

#[test]
fn reopening_a_peer_starts_from_a_clean_page() {
    let mut model = LinkedDevices::default();
    model.set_linked(None, vec![device("phone")], None);
    model.open_history(0);
    model.set_history("phone", vec![event(1)], None);
    model.close_history();
    assert_eq!(model.open_history(0).as_deref(), Some("phone"));
    assert!(model.devices()[0].history.is_empty());
    assert_eq!(model.open_history(5), None);
    assert!(!model.history_open());
}

#[test]
fn the_older_page_is_asked_for_once_per_cursor() {
    let mut model = LinkedDevices::default();
    model.set_linked(None, vec![device("phone")], None);
    model.open_history(0);
    // Still loading the first page: nothing more is asked.
    assert_eq!(model.take_older_history(), None);
    model.set_history("phone", vec![event(5)], Some(5));
    assert_eq!(model.take_older_history(), Some(("phone".into(), 5)));
    assert!(model.history_loading());
    assert_eq!(model.take_older_history(), None);
    // The last page has no cursor.
    model.set_history("phone", vec![event(4)], None);
    assert_eq!(model.take_older_history(), None);
}

#[test]
fn the_relay_switch_flips_optimistically() {
    let mut model = LinkedDevices::default();
    assert_eq!(model.toggle_relay(), None);
    model.set_linked(None, Vec::new(), Some(relay(true)));
    assert_eq!(model.toggle_relay(), Some(false));
    let off = model.relay().unwrap();
    assert_eq!(
        (off.enabled, off.state.as_str(), off.host.as_deref()),
        (false, "off", None)
    );
    assert_eq!(model.toggle_relay(), Some(true));
    assert_eq!(model.relay().unwrap().state, "connecting");
}

#[test]
fn presence_refresh_fires_on_its_deadline_and_stops_on_reset() {
    let mut model = LinkedDevices::default();
    assert_eq!(model.presence_deadline(), None);
    let now = Instant::now();
    model.watch_presence(now);
    let deadline = model.presence_deadline().unwrap();
    assert_eq!(deadline, now + PRESENCE_REFRESH);

    model.maintain(deadline - Duration::from_millis(1));
    assert!(!model.take_presence_refresh());
    model.maintain(deadline);
    assert!(model.take_presence_refresh());
    assert!(!model.take_presence_refresh());
    assert_eq!(model.presence_deadline(), Some(deadline + PRESENCE_REFRESH));

    model.set_linked(None, vec![device("phone")], None);
    model.open_history(0);
    model.reset_view();
    assert_eq!(model.presence_deadline(), None);
    assert!(!model.history_open());
    assert_eq!(model.devices().len(), 1, "the devices themselves are kept");
}
