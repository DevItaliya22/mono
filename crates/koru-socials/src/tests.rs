// SPDX-License-Identifier: MPL-2.0

use super::*;

#[test]
fn publication_links_require_provider_identifiers_not_remote_urls() {
    assert_eq!(
        publication_url(Network::Reddit, "t3_abc123").as_deref(),
        Some("https://www.reddit.com/comments/abc123")
    );
    assert_eq!(
        publication_url(Network::LinkedInPerson, "urn:li:share:123").as_deref(),
        Some("https://www.linkedin.com/feed/update/urn:li:share:123/")
    );
    for network in Network::ALL {
        for invalid in [
            "https://untrusted.example",
            "123/../../elsewhere",
            "",
            "t3_a?redirect=elsewhere",
            "urn:li:share:123#bad",
        ] {
            assert!(publication_url(network, invalid).is_none());
        }
    }
}

#[test]
fn schema_two_reads_and_upgrades_frozen_target_without_changing_evidence() {
    let mut post = Post::draft(123);
    let mut d = Destination::new(Channel::new(Network::Reddit, "Reddit".into()));
    d.subreddit = "r/test".into();
    d.reddit_title = "Frozen title".into();
    d.delivery.target = Some(d.target());
    d.delivery.content = Some(vec!["Original body".into()]);
    post.destinations.push(d);
    let doc = Document::new(&post).unwrap();
    doc.loro.get_map("social").insert("schema", 2_i64).unwrap();
    assert_eq!(doc.read().unwrap(), post);
    doc.write(&post).unwrap();
    let restored = Document::from_snapshot(&doc.snapshot().unwrap()).unwrap();
    assert_eq!(restored.read().unwrap(), post);
}

#[test]
fn oversized_grapheme_is_preserved_and_rejected() {
    let text = format!("a{}", "\u{0301}".repeat(300));
    let parts = split_thread(&text);
    assert_eq!(parts.len(), 1);
    assert_eq!(parts[0], format!("1/ {text}"));
    let mut post = Post::draft(0);
    post.body = text;
    post.destinations
        .push(Destination::new(Channel::new(Network::X, "X".into())));
    assert!(
        validate(&post, 0)
            .iter()
            .any(|(_, issue)| *issue == Issue::TooLong)
    );
}

#[test]
fn a_long_url_is_kept_whole_in_a_thread() {
    let url = format!("https://example.com/{}", "a".repeat(100_000));
    let body = format!("{url} {}", "b".repeat(300));
    let parts = split_thread(&body);
    assert!(parts[0].contains(&url));
    assert!(parts.iter().all(|p| weighted_length(p) <= 280));
    assert!(parts.len() >= 2);
}
use uuid::Uuid;

fn scheduled() -> Post {
    let mut post = Post::draft(100);
    post.body = "A useful update".into();
    post.destinations
        .push(Destination::new(Channel::new(Network::X, "My X".into())));
    post.scheduled_at_ms = Some(200);
    post.scheduled = true;
    post.execution_device = Some(Uuid::new_v4());
    post
}

#[test]
fn variants_follow_until_edited_and_reset_restores_following() {
    let mut post = scheduled();
    assert_eq!(post.destinations[0].body(&post.body), "A useful update");
    post.destinations[0].override_body = Some("Custom".into());
    post.body = "Changed".into();
    assert_eq!(post.destinations[0].body(&post.body), "Custom");
    post.destinations[0].override_body = None;
    assert_eq!(post.destinations[0].body(&post.body), "Changed");
}

#[test]
fn interrupted_publication_is_not_retried_and_frozen_text_survives() {
    let mut post = scheduled();
    let channel = post.destinations[0].channel.id;
    let owner = post.execution_device.unwrap();
    assert!(!post.begin_attempt(channel, Uuid::new_v4(), 300));
    assert!(!post.begin_attempt(channel, owner, 199));
    assert!(post.begin_attempt(channel, owner, 200));
    assert!(!post.begin_attempt(channel, owner, 201));
    let document = Document::new(&post).unwrap();
    let mut restored = Document::from_snapshot(&document.snapshot().unwrap())
        .unwrap()
        .read()
        .unwrap();
    assert!(restored.recover_interrupted());
    assert_eq!(restored.status(300), Status::Unconfirmed);
    assert!(!restored.begin_attempt(channel, owner, 300));
    assert_eq!(
        restored.destinations[0].delivery.content.as_ref().unwrap()[0],
        "A useful update"
    );
}

#[test]
fn snapshot_keeps_draft_overrides_source_and_identity() {
    let mut post = scheduled();
    post.source = Some(Source {
        workspace: Uuid::new_v4(),
        document: Uuid::new_v4(),
        title: "Source".into(),
        follows: true,
    });
    post.destinations[0].override_body = Some("日本語 👩‍💻".into());
    let doc = Document::new(&post).unwrap();
    assert_eq!(
        Document::from_snapshot(&doc.snapshot().unwrap())
            .unwrap()
            .read()
            .unwrap(),
        post
    );
}

#[test]
fn newer_content_is_never_rewritten() {
    let doc = Document::new(&scheduled()).unwrap();
    doc.loro
        .get_map("social")
        .insert("schema", SCHEMA_VERSION + 1)
        .unwrap();
    assert!(matches!(doc.read(), Err(DocumentError::UnsupportedVersion)));
    assert!(matches!(
        doc.write(&scheduled()),
        Err(DocumentError::UnsupportedVersion)
    ));
}

#[test]
fn schema_one_reads_default_intent_fields_and_upgrades_only_when_written() {
    let expected = scheduled();
    let mut fields = serde_json::to_value(&expected).unwrap();
    let fields = fields.as_object_mut().unwrap();
    fields.remove("retry_request");
    fields.remove("cancel_requested");
    let legacy = Document {
        loro: loro::LoroDoc::new(),
    };
    let map = legacy.loro.get_map("social");
    map.insert("schema", 1_i64).unwrap();
    for (key, value) in fields {
        map.insert(key.as_str(), serde_json::to_string(value).unwrap())
            .unwrap();
    }
    let restored = Document::from_snapshot(&legacy.snapshot().unwrap()).unwrap();
    let read = restored.read().unwrap();
    assert_eq!(read, expected);
    assert!(!read.cancel_requested);
    assert_eq!(read.retry_request, None);
    let loro::LoroValue::Map(before) = restored.loro.get_map("social").get_deep_value() else {
        panic!("social root must remain a map");
    };
    assert_eq!(before.get("schema"), Some(&loro::LoroValue::I64(1)));

    restored.write(&read).unwrap();
    let loro::LoroValue::Map(after) = restored.loro.get_map("social").get_deep_value() else {
        panic!("social root must remain a map");
    };
    assert_eq!(SCHEMA_VERSION, 3);
    assert_eq!(
        after.get("schema"),
        Some(&loro::LoroValue::I64(SCHEMA_VERSION))
    );
    assert_eq!(restored.read().unwrap(), expected);
}

#[test]
fn persisted_cancellation_prevents_editing_and_beginning_a_delivery() {
    let mut post = scheduled();
    post.cancel_requested = true;
    let document = Document::new(&post).unwrap();
    let restored = Document::from_snapshot(&document.snapshot().unwrap())
        .unwrap()
        .read()
        .unwrap();
    assert_eq!(restored, post);
    assert_eq!(restored.status(199), Status::CancelRequested);
    assert_eq!(restored.status(300), Status::CancelRequested);
    assert!(!restored.editable());
    let owner = restored.execution_device.unwrap();
    assert!(!restored.due_on(owner, 300));
    let channel = post.destinations[0].channel.id;
    assert!(!post.begin_attempt(channel, owner, 300));
    assert!(post.destinations[0].delivery.content.is_none());
}

#[test]
fn future_work_and_partial_delivery_have_honest_status() {
    let mut post = scheduled();
    assert_eq!(post.status(199), Status::Scheduled);
    assert_eq!(post.status(200), Status::Queued);
    post.destinations[0].delivery.published.push(Publication {
        id: "first-thread-part".into(),
        url: None,
        published_at_ms: 200,
    });
    post.destinations[0].delivery.state = DeliveryState::Failed;
    assert_eq!(post.status(200), Status::Partial);
    assert!(!post.editable());
}

#[test]
fn reddit_flair_is_required_only_when_its_rules_require_it() {
    let mut post = scheduled();
    post.destinations[0].channel.network = Network::Reddit;
    post.destinations[0].subreddit = "r/SideProject".into();
    post.destinations[0].reddit_title = "An update".into();
    assert!(validate(&post, 100).is_empty());
    post.destinations[0].flair_required = true;
    assert!(
        validate(&post, 100)
            .iter()
            .any(|(_, issue)| *issue == Issue::MissingFlair)
    );
    post.destinations[0].flair_id = Some("provider-flair-id".into());
    assert!(validate(&post, 100).is_empty());
}

#[test]
fn thread_splitting_preserves_unicode_and_handles_long_words() {
    for body in [
        "word ".repeat(400),
        "日".repeat(600),
        "👩‍💻".repeat(200),
        "a".repeat(3000),
    ] {
        let parts = split_thread(&body);
        assert!(parts.len() > 1);
        assert!(parts.iter().all(|part| weighted_length(part) <= 280));
        let restored: String = parts
            .iter()
            .map(|part| part.split_once("/ ").unwrap().1)
            .collect();
        assert_eq!(restored.replace(' ', ""), body.replace(' ', ""));
    }
    assert_eq!(weighted_length("https://example.com/a/long/link"), 23);
    assert_eq!(weighted_length("👩‍💻"), 2);
}
