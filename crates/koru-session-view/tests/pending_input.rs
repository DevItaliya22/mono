// SPDX-License-Identifier: MPL-2.0

use std::path::PathBuf;
use std::sync::{Arc, Mutex};

use koru_draw::geometry::Rect;
use koru_session_protocol::{
    AgentTurnId, Attachment, AttachmentDeliveryResult, AttachmentId, AttachmentKind,
    AttachmentSource, AttachmentStatus, DeliveryMode, DeliveryState, ErrorCode, ImageDimensions,
    SendPendingInputResult, SessionFailure, SessionId,
};
use koru_session_view::{
    COMPOSER_HEIGHT, MAX_DRAFT_CHARS, PendingAttachmentStatus, PendingInputCopy, PendingInputHit,
    PendingInputIntent, PendingInputView, SessionSurfaceIntent, SessionSurfaceInteraction,
};

#[derive(Clone, Default)]
struct Recorded(Arc<Mutex<Vec<PendingInputIntent>>>);

impl SessionSurfaceInteraction for Recorded {
    fn dispatch(&mut self, intent: SessionSurfaceIntent) {
        let SessionSurfaceIntent::PendingInput(intent) = intent else {
            panic!("pending-input view emitted a different surface intent");
        };
        self.0.lock().unwrap().push(intent);
    }
}

struct TestHost {
    commands: Recorded,
    view: PendingInputView<Recorded>,
    area: Rect,
}

impl TestHost {
    fn attached(attachments: Vec<Attachment>) -> Self {
        let commands = Recorded::default();
        let mut view = PendingInputView::new(commands.clone(), PendingInputCopy::default());
        view.attach(session(), attachments);
        Self {
            commands,
            view,
            area: Rect::new(0.0, 0.0, 640.0, COMPOSER_HEIGHT),
        }
    }

    fn click(&mut self, hit: PendingInputHit) {
        let layout = self.view.layout(self.area);
        let rect = match hit {
            PendingInputHit::Draft => layout.draft,
            PendingInputHit::Attach => layout.attach,
            PendingInputHit::Send | PendingInputHit::Retry => layout.send,
            PendingInputHit::Remove(id) => {
                layout
                    .attachments
                    .iter()
                    .find(|item| item.id == id)
                    .unwrap()
                    .remove
            }
        };
        let (x, y) = rect.centre();
        assert!(self.view.click(self.area, x, y));
    }

    fn take_commands(&self) -> Vec<PendingInputIntent> {
        std::mem::take(&mut *self.commands.0.lock().unwrap())
    }
}

fn session() -> SessionId {
    "11111111-1111-4111-8111-111111111111".parse().unwrap()
}

fn attachment(id: AttachmentId, mode: DeliveryMode, state: DeliveryState) -> Attachment {
    Attachment {
        id,
        session_id: session(),
        kind: AttachmentKind::Image,
        source: AttachmentSource::Clipboard,
        media_type: "image/png".into(),
        byte_count: 64,
        dimensions: Some(ImageDimensions {
            width: 4,
            height: 4,
        }),
        status: AttachmentStatus::Pending,
        delivery_mode: mode,
        delivery: state,
        turn_id: None,
        preview_available: true,
        created_at_ms: 1,
    }
}

fn sent(id: AttachmentId, mode: DeliveryMode, state: DeliveryState) -> SendPendingInputResult {
    SendPendingInputResult {
        turn_id: AgentTurnId::mint(),
        attachments: vec![AttachmentDeliveryResult {
            attachment_id: id,
            mode,
            state,
        }],
    }
}

#[test]
fn another_surface_can_host_text_only_state_and_receive_the_typed_command() {
    let mut host = TestHost::attached(Vec::new());
    host.click(PendingInputHit::Draft);
    assert!(host.view.insert_text("hello \u{1f30d}"));
    host.click(PendingInputHit::Send);
    let commands = host.take_commands();
    let PendingInputIntent::Send { input } = &commands[0] else {
        panic!("expected send");
    };
    assert_eq!(input.text, "hello \u{1f30d}");
    assert!(input.attachment_ids.is_empty());
}

#[test]
fn one_and_many_images_have_bounded_geometry_and_ordered_submission() {
    let ids = (0..12).map(|_| AttachmentId::mint()).collect::<Vec<_>>();
    let attachments = ids
        .iter()
        .map(|id| attachment(*id, DeliveryMode::QuotedPath, DeliveryState::Pending))
        .collect();
    let mut host = TestHost::attached(attachments);
    let layout = host.view.layout(host.area);
    assert!(!layout.attachments.is_empty());
    assert!(layout.attachments.len() < ids.len());
    assert!(layout.overflow.is_some());
    host.click(PendingInputHit::Send);
    let PendingInputIntent::Send { input } = host.take_commands().remove(0) else {
        panic!("expected send");
    };
    assert_eq!(input.attachment_ids, ids);
}

#[test]
fn ingest_progress_limit_and_removal_are_explicit() {
    let id = AttachmentId::mint();
    let mut host = TestHost::attached(Vec::new());
    host.view.begin_ingest(id, AttachmentSource::Drop);
    assert_eq!(
        host.view.model().attachments()[0].status,
        PendingAttachmentStatus::Ingesting
    );
    host.view.ingest_failed(id, ErrorCode::AttachmentLimit);
    assert_eq!(host.view.model().error(), Some(&ErrorCode::AttachmentLimit));
    host.click(PendingInputHit::Remove(id));
    assert!(host.take_commands().is_empty());
    assert!(host.view.model().attachments().is_empty());
}

#[test]
fn removing_a_stored_attachment_emits_the_surface_neutral_command() {
    let id = AttachmentId::mint();
    let mut host = TestHost::attached(vec![attachment(
        id,
        DeliveryMode::QuotedPath,
        DeliveryState::Pending,
    )]);
    host.click(PendingInputHit::Remove(id));
    assert_eq!(
        host.take_commands(),
        vec![PendingInputIntent::RemoveAttachment { attachment_id: id }]
    );
    // The host remains authoritative: the row stays visible until its
    // asynchronous removal result returns.
    assert_eq!(host.view.model().attachments()[0].id, id);
}

#[test]
fn an_uncertain_text_only_failure_retries_the_exact_idempotency_key() {
    let mut host = TestHost::attached(Vec::new());
    assert!(host.view.insert_text("possibly accepted"));
    host.click(PendingInputHit::Send);
    let PendingInputIntent::Send { input: original } = host.take_commands().remove(0) else {
        panic!("expected send");
    };
    host.view.submission_failed(ErrorCode::HostUnavailable);
    assert!(host.view.model().has_retry());
    host.click(PendingInputHit::Retry);
    let PendingInputIntent::Retry { input: retry } = host.take_commands().remove(0) else {
        panic!("expected retry");
    };
    assert_eq!(retry, original);
}

#[test]
fn native_path_and_unsupported_delivery_remain_visible_in_view_state() {
    for mode in [
        DeliveryMode::NativeStructured,
        DeliveryMode::QuotedPath,
        DeliveryMode::Unsupported,
    ] {
        let id = AttachmentId::mint();
        let host = TestHost::attached(vec![attachment(id, mode, DeliveryState::Pending)]);
        assert_eq!(host.view.model().attachments()[0].delivery_mode, mode);
        assert_eq!(
            host.view.model().attachments()[0].delivery,
            DeliveryState::Pending
        );
    }
}

#[test]
fn partial_failure_retries_the_identical_logical_input() {
    let failed = AttachmentId::mint();
    let delivered = AttachmentId::mint();
    let mut host = TestHost::attached(vec![
        attachment(failed, DeliveryMode::QuotedPath, DeliveryState::Pending),
        attachment(delivered, DeliveryMode::QuotedPath, DeliveryState::Pending),
    ]);
    assert!(host.view.insert_text("inspect"));
    host.click(PendingInputHit::Send);
    let PendingInputIntent::Send { input: original } = host.take_commands().remove(0) else {
        panic!("expected send");
    };
    let turn = AgentTurnId::mint();
    host.view.submission_succeeded(SendPendingInputResult {
        turn_id: turn,
        attachments: vec![
            AttachmentDeliveryResult {
                attachment_id: failed,
                mode: DeliveryMode::QuotedPath,
                state: DeliveryState::Failed {
                    turn_id: turn,
                    error: SessionFailure {
                        code: ErrorCode::DeliveryFailed,
                        message: "content-free".into(),
                    },
                },
            },
            AttachmentDeliveryResult {
                attachment_id: delivered,
                mode: DeliveryMode::QuotedPath,
                state: DeliveryState::Delivered { turn_id: turn },
            },
        ],
    });
    assert!(host.view.model().has_retry());
    host.click(PendingInputHit::Retry);
    let PendingInputIntent::Retry { input: retry } = host.take_commands().remove(0) else {
        panic!("expected retry");
    };
    assert_eq!(retry, original);
}

#[test]
fn detach_and_reattach_restore_host_owned_pending_attachments_not_a_stale_draft() {
    let id = AttachmentId::mint();
    let stored = attachment(id, DeliveryMode::NativeStructured, DeliveryState::Pending);
    let mut host = TestHost::attached(vec![stored.clone()]);
    assert!(host.view.insert_text("surface-local draft"));
    host.view.detach();
    assert!(host.view.model().attachments().is_empty());
    assert!(host.view.model().draft().is_empty());
    host.view.attach(session(), vec![stored]);
    assert_eq!(host.view.model().attachments()[0].id, id);
}

#[test]
fn dropped_files_and_bitmap_acquisition_never_enter_terminal_bytes() {
    let mut host = TestHost::attached(Vec::new());
    assert!(
        host.view
            .dropped_files(vec![PathBuf::from("image with spaces.png")])
    );
    host.click(PendingInputHit::Attach);
    assert_eq!(
        host.take_commands(),
        vec![
            PendingInputIntent::AddFiles {
                paths: vec![PathBuf::from("image with spaces.png")]
            },
            PendingInputIntent::AcquireClipboardImage,
        ]
    );
}

#[test]
fn composer_clipboard_paste_is_a_typed_host_job_and_busy_state_rejects_new_sources() {
    let id = AttachmentId::mint();
    let mut host = TestHost::attached(vec![attachment(
        id,
        DeliveryMode::QuotedPath,
        DeliveryState::Pending,
    )]);
    assert!(host.view.paste_clipboard());
    assert_eq!(
        host.take_commands(),
        vec![PendingInputIntent::PasteClipboard]
    );

    host.click(PendingInputHit::Send);
    host.take_commands();
    assert!(!host.view.paste_clipboard());
    assert!(!host.view.dropped_files(vec![PathBuf::from("late.png")]));
}

#[test]
fn draft_work_is_bounded_before_layout() {
    let mut host = TestHost::attached(Vec::new());
    assert!(host.view.insert_text(&"界".repeat(MAX_DRAFT_CHARS + 10)));
    assert_eq!(host.view.model().draft().chars().count(), MAX_DRAFT_CHARS);
    assert_eq!(host.view.model().error(), Some(&ErrorCode::InputTooLarge));
}

#[test]
fn delivered_and_failed_results_are_applied_without_losing_the_mode() {
    let id = AttachmentId::mint();
    let mut host = TestHost::attached(vec![attachment(
        id,
        DeliveryMode::NativeStructured,
        DeliveryState::Pending,
    )]);
    host.click(PendingInputHit::Send);
    host.take_commands();
    let turn = AgentTurnId::mint();
    host.view.submission_succeeded(sent(
        id,
        DeliveryMode::NativeStructured,
        DeliveryState::Delivered { turn_id: turn },
    ));
    assert_eq!(
        host.view.model().attachments()[0].delivery,
        DeliveryState::Delivered { turn_id: turn }
    );
    assert_eq!(
        host.view.model().attachments()[0].delivery_mode,
        DeliveryMode::NativeStructured
    );
}
