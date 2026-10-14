// SPDX-License-Identifier: MPL-2.0

use super::*;

fn key() -> Key {
    Key {
        workspace: Uuid::from_u128(1),
        post: Uuid::from_u128(2),
        channel: Uuid::from_u128(3),
    }
}

fn publication(id: &str) -> Publication {
    Publication {
        id: id.into(),
        url: Some(format!("https://x.com/i/status/{id}")),
        published_at_ms: 123_456,
    }
}

fn attempt() -> Attempt {
    Attempt {
        target: koru_socials::PublicationTarget::X,
        id: Uuid::from_u128(4),
        device: Uuid::from_u128(5),
        actor: "actor-1".into(),
        parts: vec![
            "First frozen part — café".into(),
            "Second frozen part".into(),
        ],
        published: vec![publication("remote-1")],
        phase: Phase::Ready,
        next_at_ms: Some(1_000_000),
        failures: 2,
        request: Some(Uuid::from_u128(6)),
        error: Some("retry_wait".into()),
    }
}

fn ledger() -> SqliteLedger {
    SqliteLedger::from_connection(Connection::open_in_memory().unwrap()).unwrap()
}

#[test]
fn attempt_roundtrip_preserves_frozen_text_confirmation_and_retry_metadata() {
    let mut ledger = ledger();
    assert_eq!(ledger.load(key()), Ok(None));
    let expected = attempt();
    assert_eq!(ledger.store(key(), &expected), Ok(()));
    assert_eq!(ledger.load(key()), Ok(Some(expected.clone())));

    // Reopen the initialized connection, retaining only the in-memory database.
    let mut reopened = SqliteLedger::from_connection(ledger.db).unwrap();
    assert_eq!(reopened.load(key()), Ok(Some(expected)));
    for other in [
        Key {
            workspace: Uuid::from_u128(99),
            ..key()
        },
        Key {
            post: Uuid::from_u128(99),
            ..key()
        },
        Key {
            channel: Uuid::from_u128(99),
            ..key()
        },
    ] {
        assert_eq!(
            reopened.load(other),
            Ok(None),
            "each scope component isolates delivery evidence"
        );
    }
}

#[test]
fn frozen_content_attempt_device_and_actor_cannot_be_replaced() {
    let mut ledger = ledger();
    let original = attempt();
    ledger.store(key(), &original).unwrap();
    let mutations: [fn(&mut Attempt); 7] = [
        |a| a.target = PublicationTarget::LinkedInPerson,
        |a| a.id = Uuid::from_u128(99),
        |a| a.device = Uuid::from_u128(99),
        |a| a.actor = "actor-2".into(),
        |a| a.parts[0] = "replacement content".into(),
        |a| {
            a.parts.pop();
        },
        |a| a.parts.push("extra part".into()),
    ];
    for mutate in mutations {
        let mut replacement = original.clone();
        mutate(&mut replacement);
        assert_eq!(ledger.store(key(), &replacement), Err(Fault::Ledger));
        assert_eq!(
            ledger.load(key()),
            Ok(Some(original.clone())),
            "rejected writes must preserve the durable original"
        );
    }
}

#[test]
fn confirmed_prefix_can_extend_but_cannot_regress_or_change() {
    let mut ledger = ledger();
    let original = attempt();
    ledger.store(key(), &original).unwrap();
    let mutations: [fn(&mut Attempt); 4] = [
        |a| a.published.clear(),
        |a| a.published[0].id = "different-id".into(),
        |a| a.published[0].url = None,
        |a| a.published[0].published_at_ms += 1,
    ];
    for mutate in mutations {
        let mut replacement = original.clone();
        mutate(&mut replacement);
        assert_eq!(ledger.store(key(), &replacement), Err(Fault::Ledger));
        assert_eq!(ledger.load(key()), Ok(Some(original.clone())));
    }

    let mut complete = original;
    complete.published.push(publication("remote-2"));
    complete.phase = Phase::Published;
    complete.next_at_ms = None;
    complete.error = None;
    complete.failures = 0;
    ledger.store(key(), &complete).unwrap();
    assert_eq!(ledger.load(key()), Ok(Some(complete.clone())));
    let mut impossible = complete.clone();
    impossible
        .published
        .push(publication("no-corresponding-part"));
    assert_eq!(ledger.store(key(), &impossible), Err(Fault::Ledger));
    assert_eq!(ledger.load(key()), Ok(Some(complete)));
}

#[test]
fn future_schema_is_rejected_before_the_ledger_is_available() {
    let db = Connection::open_in_memory().unwrap();
    db.pragma_update(None, "user_version", 3).unwrap();
    assert!(matches!(
        SqliteLedger::from_connection(db),
        Err(Fault::Ledger)
    ));
}

#[test]
fn reddit_metadata_survives_reopen_and_cannot_change_on_retry() {
    let mut ledger = ledger();
    let mut original = attempt();
    original.target = PublicationTarget::Reddit {
        subreddit: "test".into(),
        title: "Frozen title".into(),
        flair_id: Some("flair-1".into()),
    };
    ledger.store(key(), &original).unwrap();
    let mut reopened = SqliteLedger::from_connection(ledger.db).unwrap();
    assert_eq!(reopened.load(key()), Ok(Some(original.clone())));
    for target in [
        PublicationTarget::Reddit {
            subreddit: "different".into(),
            title: "Frozen title".into(),
            flair_id: Some("flair-1".into()),
        },
        PublicationTarget::Reddit {
            subreddit: "test".into(),
            title: "Changed title".into(),
            flair_id: Some("flair-1".into()),
        },
        PublicationTarget::Reddit {
            subreddit: "test".into(),
            title: "Frozen title".into(),
            flair_id: None,
        },
    ] {
        let mut changed = original.clone();
        changed.target = target;
        assert_eq!(reopened.store(key(), &changed), Err(Fault::Ledger));
        assert_eq!(reopened.load(key()), Ok(Some(original.clone())));
    }
}

#[test]
fn schema_one_migration_preserves_confirmed_x_evidence() {
    let db = Connection::open_in_memory().unwrap();
    db.execute_batch(include_str!("../../migrations/socials_delivery_v1.sql"))
        .unwrap();
    let k = key();
    let a = attempt();
    db.execute("INSERT INTO social_attempts(workspace,post,channel,attempt,device,actor,phase,failures) VALUES(?1,?2,?3,?4,?5,'actor-1','ready',0)",params![k.workspace.as_bytes().as_slice(),k.post.as_bytes().as_slice(),k.channel.as_bytes().as_slice(),a.id.as_bytes().as_slice(),a.device.as_bytes().as_slice()]).unwrap();
    db.execute("INSERT INTO social_parts(workspace,post,channel,part,text,remote_id,published_at_ms) VALUES(?1,?2,?3,0,'Original','123',42)",params![k.workspace.as_bytes().as_slice(),k.post.as_bytes().as_slice(),k.channel.as_bytes().as_slice()]).unwrap();
    let mut migrated = SqliteLedger::from_connection(db).unwrap();
    let saved = migrated.load(k).unwrap().unwrap();
    assert_eq!(saved.target, PublicationTarget::X);
    assert_eq!(saved.id, a.id);
    assert_eq!(saved.parts, ["Original"]);
    assert_eq!(saved.published[0].id, "123");
    assert_eq!(saved.published[0].published_at_ms, 42);
}
