// SPDX-License-Identifier: MPL-2.0

use std::{fs, path::PathBuf};

use koru_contract::proto::*;
use prost::Message;
use prost_types::Any;

const DOC: &str = "10000000-0000-4000-8000-000000000001";
const WORKSPACE: &str = "10000000-0000-4000-8000-000000000002";
const ROOT: &str = "10000000-0000-4000-8000-000000000003";
const CHILD: &str = "10000000-0000-4000-8000-000000000004";
const INLINE: &str = "10000000-0000-4000-8000-000000000005";

fn main() {
    let root = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../contracts/testdata");
    for directory in ["valid", "invalid", "forward_compat"] {
        fs::create_dir_all(root.join(directory)).unwrap();
    }
    account_enrollment_fixtures(&root);
    let start = koru_contract::mobile_proto::AccountCommandRequest {
        version: 1,
        requester: 7,
        action: koru_contract::mobile_proto::AccountAction::StartGoogle as i32,
        locale: "en".into(),
        expected_owner_scope: Some(0),
        ..Default::default()
    };
    write(&root, "valid/account_start.pb", &start);
    let mut future = start.encode_to_vec();
    // Unknown varint field 100 remains harmless to older clients.
    future.extend_from_slice(&[0xa0, 0x06, 0x01]);
    fs::write(
        root.join("forward_compat/account_start_unknown_field.pb"),
        future,
    )
    .unwrap();
    for (name, action) in [
        (
            "confirm",
            koru_contract::mobile_proto::AccountAction::ConfirmAccountCreation,
        ),
        (
            "cancel",
            koru_contract::mobile_proto::AccountAction::CancelAccountCreation,
        ),
    ] {
        write(
            &root,
            &format!("valid/account_creation_{name}.pb"),
            &koru_contract::mobile_proto::AccountCommandRequest {
                version: 1,
                requester: 7,
                action: action as i32,
                expected_owner_scope: Some(0),
                ..Default::default()
            },
        );
    }
    write(
        &root,
        "valid/account_empty_profiles.pb",
        &koru_contract::mobile_proto::MobileAccountResult {
            sequence: 1,
            requester: 2,
            generation: 2,
            kind: "profiles".into(),
            account: Some(koru_contract::mobile_proto::MobileAccountSummary {
                display_name: "Existing account".into(),
            }),
            ..Default::default()
        },
    );

    write(
        &root,
        "valid/simple_document.pb",
        &document("A simple document", "Hello, Koru."),
    );
    write(
        &root,
        "valid/multilingual_document.pb",
        &document("Multilingual", "Hello · مرحبا · Olá · 你好 · こんにちは 👋"),
    );
    write(
        &root,
        "valid/rtl_document.pb",
        &document("مستند Koru", "مرحبا بالعالم — Koru 123"),
    );

    let mut image = document("Image", "An image follows.");
    image.blocks.push(BlockEnvelope {
        id: Some(BlockId {
            value: CHILD.into(),
        }),
        parent_id: None,
        kind: BlockKind::Image as i32,
        properties: Some(BlockProperties::default()),
        payload: Some(pack(
            "koru.v1.ImagePayload",
            &ImagePayload {
                blob: Some(BlobReference {
                    id: Some(BlobId {
                        value: "sha256:4a6f7275".into(),
                    }),
                    mime_type: "image/png".into(),
                    size: 4,
                    width: Some(1),
                    height: Some(1),
                }),
            },
        )),
    });
    write(&root, "valid/image_document.pb", &image);

    let mut invalid_uuid = document("Invalid UUID", "Rejected");
    invalid_uuid.id.as_mut().unwrap().value = "not-a-uuid".into();
    write(&root, "invalid/invalid_uuid.pb", &invalid_uuid);

    let mut invalid_range = document("Invalid range", "é");
    let block = &mut invalid_range.blocks[0];
    let mut paragraph =
        ParagraphPayload::decode(block.payload.as_ref().unwrap().value.as_slice()).unwrap();
    paragraph.rich_text.as_mut().unwrap().marks.push(TextMark {
        range: Some(TextRange {
            start_utf8: 1,
            end_utf8: 2,
        }),
        kind: "bold".into(),
        value: Vec::new(),
    });
    block.payload.as_mut().unwrap().value = paragraph.encode_to_vec();
    write(&root, "invalid/invalid_range.pb", &invalid_range);

    let mut invalid_hierarchy = document("Invalid hierarchy", "Child before parent");
    invalid_hierarchy.blocks[0].parent_id = Some(BlockId {
        value: CHILD.into(),
    });
    invalid_hierarchy
        .blocks
        .push(paragraph_block(CHILD, "Parent"));
    write(&root, "invalid/invalid_hierarchy.pb", &invalid_hierarchy);

    let mut invalid_required = document("", "Missing payload");
    invalid_required.blocks[0].payload = None;
    write(
        &root,
        "invalid/invalid_required_fields.pb",
        &invalid_required,
    );

    let mut unknown_block = document("Future block", "Known neighbor");
    unknown_block.blocks.push(BlockEnvelope {
        id: Some(BlockId {
            value: CHILD.into(),
        }),
        parent_id: None,
        kind: 777,
        properties: Some(BlockProperties::default()),
        payload: Some(Any {
            type_url: "type.koru.do/koru.v9.FutureBlockPayload".into(),
            value: vec![0x08, 0x96, 0x01],
        }),
    });
    write(
        &root,
        "forward_compat/unknown_block_kind.pb",
        &unknown_block,
    );

    let mut unknown_mark = document("Future mark", "marked");
    let block = &mut unknown_mark.blocks[0];
    let mut paragraph =
        ParagraphPayload::decode(block.payload.as_ref().unwrap().value.as_slice()).unwrap();
    paragraph.rich_text.as_mut().unwrap().marks.push(TextMark {
        range: Some(TextRange {
            start_utf8: 0,
            end_utf8: 6,
        }),
        kind: "koru.future.sparkle".into(),
        value: vec![0xca, 0xfe],
    });
    block.payload.as_mut().unwrap().value = paragraph.encode_to_vec();
    write(&root, "forward_compat/unknown_mark.pb", &unknown_mark);

    let unknown_event = DocumentEventEnvelope {
        document_id: Some(DocumentId { value: DOC.into() }),
        sequence: 42,
        event: Some(document_event_envelope::Event::Extension(Any {
            type_url: "type.koru.do/koru.v9.FutureEvent".into(),
            value: vec![0x12, 0x03, b'n', b'e', b'w'],
        })),
    };
    write(&root, "forward_compat/unknown_event.pb", &unknown_event);
}

fn document(title: &str, text: &str) -> DocumentProjection {
    DocumentProjection {
        id: Some(DocumentId { value: DOC.into() }),
        workspace_id: Some(WorkspaceId {
            value: WORKSPACE.into(),
        }),
        title: title.into(),
        revision: 1,
        blocks: vec![paragraph_block(ROOT, text)],
    }
}

fn paragraph_block(id: &str, text: &str) -> BlockEnvelope {
    let inline_offset = text.find('\u{fffc}').map(|offset| offset as u32);
    BlockEnvelope {
        id: Some(BlockId { value: id.into() }),
        parent_id: None,
        kind: BlockKind::Paragraph as i32,
        properties: Some(BlockProperties::default()),
        payload: Some(pack(
            "koru.v1.ParagraphPayload",
            &ParagraphPayload {
                rich_text: Some(RichText {
                    text: text.into(),
                    marks: Vec::new(),
                    inline_objects: inline_offset
                        .into_iter()
                        .map(|anchor_utf8| InlineObject {
                            id: Some(InlineObjectId {
                                value: INLINE.into(),
                            }),
                            attribute_id: None,
                            anchor_utf8,
                            role: "document_reference".into(),
                            reference: Some(EntityReference {
                                target: Some(entity_reference::Target::DocumentId(DocumentId {
                                    value: DOC.into(),
                                })),
                            }),
                            label: Some("Koru".into()),
                        })
                        .collect(),
                }),
            },
        )),
    }
}

fn pack(name: &str, message: &impl Message) -> Any {
    Any {
        type_url: format!("type.googleapis.com/{name}"),
        value: message.encode_to_vec(),
    }
}

fn account_enrollment_fixtures(root: &std::path::Path) {
    use koru_contract::mobile_proto as mobile;
    let select = mobile::AccountCommandRequest {
        version: 1,
        requester: 7,
        expected_owner_scope: Some(9),
        action: mobile::AccountAction::SelectProfiles as i32,
        ..Default::default()
    };
    for (name, ids) in [
        ("valid/account_select_profiles_empty.pb", vec![]),
        ("valid/account_select_profiles_one.pb", vec![DOC]),
        (
            "valid/account_select_profiles_subset.pb",
            vec![DOC, WORKSPACE],
        ),
        (
            "invalid/account_select_profiles_duplicate.pb",
            vec![DOC, DOC],
        ),
        (
            "invalid/account_select_profiles_unsorted.pb",
            vec![WORKSPACE, DOC],
        ),
        (
            "invalid/account_select_profiles_noncanonical.pb",
            vec!["10000000000040008000000000000001"],
        ),
        (
            "invalid/account_select_profiles_uuid_version.pb",
            vec!["10000000-0000-1000-8000-000000000001"],
        ),
    ] {
        write(
            root,
            name,
            &mobile::AccountCommandRequest {
                profile_ids: ids.into_iter().map(str::to_owned).collect(),
                ..select.clone()
            },
        );
    }
    write(
        root,
        "invalid/account_select_profiles_missing_scope.pb",
        &mobile::AccountCommandRequest {
            expected_owner_scope: None,
            ..select.clone()
        },
    );
    write(
        root,
        "invalid/account_select_profile_legacy.pb",
        &mobile::AccountCommandRequest {
            action: 6,
            profile_id: DOC.into(),
            ..select.clone()
        },
    );
    for (name, profile_id) in [
        ("valid/account_retry_enrollment_all.pb", ""),
        ("valid/account_retry_enrollment_one.pb", DOC),
    ] {
        write(
            root,
            name,
            &mobile::AccountCommandRequest {
                action: mobile::AccountAction::RetryEnrollment as i32,
                profile_id: profile_id.into(),
                ..select.clone()
            },
        );
    }
    write(
        root,
        "invalid/account_profile_set_wrong_action.pb",
        &mobile::AccountCommandRequest {
            action: mobile::AccountAction::RetryEnrollment as i32,
            profile_ids: vec![DOC.into()],
            ..select
        },
    );
    let rows = vec![
        mobile::MobileProfileEnrollment {
            profile_id: DOC.into(),
            stage: "waiting".into(),
            failure_code: Some("no_peer".into()),
            retry_at_millis: Some(45),
        },
        mobile::MobileProfileEnrollment {
            profile_id: WORKSPACE.into(),
            stage: "complete".into(),
            failure_code: None,
            retry_at_millis: None,
        },
        mobile::MobileProfileEnrollment {
            profile_id: ROOT.into(),
            stage: "failed".into(),
            failure_code: Some("trust_persistence_failed".into()),
            retry_at_millis: Some(90),
        },
    ];
    write(
        root,
        "valid/account_enrollment_status.pb",
        &mobile::MobileAccountStatus {
            active_scope: 9,
            generation: 7,
            stage: "signed_in".into(),
            enrollments: rows.clone(),
            ..Default::default()
        },
    );
    let result = mobile::MobileAccountResult {
        sequence: 12,
        requester: 7,
        owner_scope: 9,
        generation: 7,
        kind: "profiles_selected".into(),
        enrollments: rows,
        ..Default::default()
    };
    write(root, "valid/account_profiles_selected.pb", &result);
    let mut legacy = mobile::MobileAccountResult {
        kind: "selected".into(),
        enrollments: vec![],
        ..result
    }
    .encode_to_vec();
    // Reserved former selected_profile_id (field 7) cannot become a set row.
    legacy.extend_from_slice(&[0x3a, DOC.len() as u8]);
    legacy.extend_from_slice(DOC.as_bytes());
    fs::write(
        root.join("forward_compat/account_removed_selected_field.pb"),
        legacy,
    )
    .unwrap();
}

fn write(root: &std::path::Path, relative: &str, message: &impl Message) {
    fs::write(root.join(relative), message.encode_to_vec()).unwrap();
}
