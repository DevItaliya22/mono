// SPDX-License-Identifier: MPL-2.0

use super::*;

fn authority(token: &str) -> EnrollmentAuthorization {
    EnrollmentAuthorization {
        access: koru_account_wire::AccessToken(token.to_owned()),
        owner: crate::identity::AccountOwner {
            account: "30000000-0000-4000-8000-000000000001".parse().unwrap(),
            auth_identity: "40000000-0000-4000-8000-000000000001".parse().unwrap(),
        },
        namespace: "n".repeat(64),
        record_version: 1,
        session_id: "session-a".into(),
    }
}

fn watch() -> (SessionAccess, Arc<Mutex<Desired>>) {
    let desired = Arc::new(Mutex::new(Desired::default()));
    (
        SessionAccess {
            foreground: None,
            authority: authority("old"),
            desired: Some(desired.clone()),
            cancel: Cancel::default(),
        },
        desired,
    )
}

#[test]
fn a_blocked_session_sees_refresh_without_consuming_worker_commands() {
    let (watch, desired) = watch();
    desired
        .lock()
        .unwrap()
        .commands
        .push_back(Command::Refresh(authority("old"), authority("fresh")));
    assert_eq!(watch.access_token().unwrap().0, "fresh");
    assert_eq!(desired.lock().unwrap().commands.len(), 1);
}

#[test]
fn switching_away_and_back_cannot_revive_an_old_session() {
    let (watch, desired) = watch();
    let mut queue = desired.lock().unwrap();
    queue.commands.push_back(Command::Activate(None, Some(2)));
    queue
        .commands
        .push_back(Command::Activate(Some(authority("fresh")), Some(3)));
    drop(queue);
    assert!(watch.access_token().is_none());
}

#[test]
fn foreign_refresh_does_not_supply_authority_to_this_session() {
    let (watch, desired) = watch();
    let mut foreign = authority("foreign");
    foreign.session_id = "session-b".into();
    desired
        .lock()
        .unwrap()
        .commands
        .push_back(Command::Refresh(foreign.clone(), foreign));
    assert_eq!(watch.access_token().unwrap().0, "old");
    watch.cancel.request();
    assert!(watch.access_token().is_none());
}
