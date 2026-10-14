// SPDX-License-Identifier: MPL-2.0

//! Ids are strict v4, spelled one way, and debug-print short.

use super::*;

#[test]
fn ids_accept_only_canonical_v4_spellings() {
    let canonical = v4(0xab).hyphenated().to_string();
    assert!(canonical.parse::<RunId>().is_ok());
    assert_eq!(
        canonical.to_uppercase().parse::<RunId>().unwrap_err(),
        IdParseError::InvalidSpelling
    );
    assert_eq!(
        "00000000-0000-1000-8000-000000000001"
            .parse::<RunAttemptId>()
            .unwrap_err(),
        IdParseError::NotVersion4
    );
    assert_eq!(
        "not-a-uuid".parse::<TimelineEntryId>().unwrap_err(),
        IdParseError::InvalidSpelling
    );
    let refused: Result<UsageFactId, _> = serde_json::from_str("\"abc\"");
    assert!(refused.is_err());
}

#[test]
fn ids_serialize_as_the_storage_spelling_and_debug_short() {
    let id = run_id(0xab);
    assert_eq!(
        serde_json::to_string(&id).unwrap(),
        "\"00000000-0000-4000-8000-0000000000ab\""
    );
    assert_eq!(id.to_storage(), "00000000-0000-4000-8000-0000000000ab");
    assert_eq!(format!("{id:?}"), "run(00000000)");
    assert_eq!(format!("{:?}", attempt_id(1)), "attempt(00000000)");
    assert_eq!(
        format!("{:?}", HostInstanceId::from_uuid(v4(2))),
        "host(00000000)"
    );
    assert_ne!(RunId::mint(), RunId::mint());
}

#[test]
fn a_schedule_id_is_a_strict_v4_like_every_other_id() {
    let id = ScheduleId::from_uuid(v4(0x5c));
    assert_eq!(format!("{id:?}"), "schedule(00000000)");
    assert_eq!(id.to_storage(), "00000000-0000-4000-8000-00000000005c");
    assert_eq!(id.to_storage().parse::<ScheduleId>().unwrap(), id);
    assert_eq!(
        "00000000-0000-1000-8000-00000000005c"
            .parse::<ScheduleId>()
            .unwrap_err(),
        IdParseError::NotVersion4
    );
    assert_ne!(ScheduleId::mint(), ScheduleId::mint());
}

#[test]
fn a_turn_identity_is_the_triple() {
    let turn = TurnIdentity {
        attempt: attempt_id(1),
        turn: turn_id(2),
        originator: TurnOriginator::Human,
    };
    let json = serde_json::to_value(turn).unwrap();
    assert_eq!(json["originator"], "human");
    assert_eq!(json["attempt"], attempt_id(1).to_storage());
    let other_attempt = TurnIdentity {
        attempt: attempt_id(3),
        ..turn
    };
    assert_ne!(
        turn, other_attempt,
        "the same turn id under another attempt is another turn"
    );
}

#[test]
fn an_actor_is_an_id_and_an_environment_names_its_device() {
    let person = ActorRef::Person {
        profile: ProfileId::from_uuid(v4(1)),
    };
    assert_eq!(serde_json::to_value(person).unwrap()["kind"], "person");
    let environment = ExecutionEnvironment::local(device_id(9));
    assert_eq!(environment.device(), Some(device_id(9)));
    let json = serde_json::to_value(environment).unwrap();
    assert_eq!(json["kind"], "local_device");
}
