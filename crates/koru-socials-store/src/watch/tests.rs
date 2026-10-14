// SPDX-License-Identifier: MPL-2.0

use super::*;

#[test]
fn catch_up_and_live_updates_invalidate_the_subscribed_target() {
    let workspace = WorkspaceId::from_uuid(Uuid::from_u128(1));
    let post = Uuid::from_u128(2);
    let channel = Uuid::from_u128(3);
    let targets = [
        ("workspace:1:manifest", Change::Manifest(workspace)),
        ("document:2", Change::Post(workspace, post)),
        ("document:3", Change::Channel(workspace, channel)),
    ];
    let rooms: HashMap<_, _> = targets
        .iter()
        .map(|(room, change)| (room.to_string(), *change))
        .collect();
    for (room, expected) in targets {
        assert_eq!(
            classify(
                &rooms,
                Some(Event::Updates {
                    room: room.into(),
                    updates: vec![vec![], vec![1]],
                }),
            ),
            Ok(Some(expected))
        );
    }
}

#[test]
fn timeout_empty_and_left_room_updates_do_not_invalidate() {
    let workspace = WorkspaceId::from_uuid(Uuid::from_u128(1));
    let rooms = HashMap::from([("manifest".into(), Change::Manifest(workspace))]);
    assert_eq!(classify(&rooms, None), Ok(None));
    for (room, updates) in [
        ("manifest", vec![]),
        ("manifest", vec![vec![]]),
        ("left-room", vec![vec![1]]),
    ] {
        assert_eq!(
            classify(
                &rooms,
                Some(Event::Updates {
                    room: room.into(),
                    updates,
                }),
            ),
            Ok(None)
        );
    }
}
