// SPDX-License-Identifier: MPL-2.0

//! The joins this file makes, without a store.
//!
//! What the wire adapter *does* — a pin that answers its captured bytes
//! after the source moved on — needs a real Store and is proved end to end
//! in `apps/koru-agent/tests/delegate_worker_process.rs`. What is decided
//! here is the two stage translations and the delivery mode, and those are
//! functions of their arguments.

use koru_session_protocol::{AttachmentDeliveryPolicy, DeliveryMode, ShellQuoting};

use super::{capture_stage, delivery_of, materialize_stage, schedule_source};

#[test]
fn every_schedule_pointer_maps_to_the_same_live_context_identity() {
    use koru_delegate_context::ContextSource;
    use koru_delegate_core::{BlobHash, BlobKind, BlockNodeId, DocumentId, WorkspaceId};
    use koru_delegate_schedule::ScheduleContextPointer;

    let workspace = WorkspaceId::from_uuid(uuid::Uuid::from_u128(1));
    let document = DocumentId::from_uuid(uuid::Uuid::from_u128(2));
    let node = BlockNodeId::from_uuid(uuid::Uuid::from_u128(3));
    let blob = BlobHash::parse(&format!("b3:{}", "a".repeat(64))).unwrap();
    let pointers = [
        ScheduleContextPointer::Document {
            workspace,
            document,
        },
        ScheduleContextPointer::Block {
            workspace,
            document,
            node,
        },
        ScheduleContextPointer::Blob {
            workspace,
            blob: blob.clone(),
            blob_kind: BlobKind::Image,
        },
    ];
    assert_eq!(
        pointers.iter().map(schedule_source).collect::<Vec<_>>(),
        vec![
            ContextSource::Document {
                workspace,
                document,
            },
            ContextSource::Block {
                workspace,
                document,
                node,
            },
            ContextSource::Blob {
                workspace,
                blob,
                blob_kind: BlobKind::Image,
            },
        ]
    );
}

/// A profile that declares nothing is handed quoted paths, not refused:
/// the layout is the same under either mode and every CLI provider this
/// milestone launches can read a path.
#[test]
fn an_undeclared_delivery_mode_is_a_quoted_path_and_never_unsupported() {
    assert_eq!(delivery_of(None), DeliveryMode::QuotedPath);
    assert_eq!(
        delivery_of(Some(AttachmentDeliveryPolicy::Unsupported)),
        DeliveryMode::QuotedPath
    );
    assert_eq!(
        delivery_of(Some(AttachmentDeliveryPolicy::default())),
        DeliveryMode::QuotedPath
    );
    assert_eq!(
        delivery_of(Some(AttachmentDeliveryPolicy::QuotedPath {
            shell: ShellQuoting::PowerShell
        })),
        DeliveryMode::QuotedPath
    );
    assert_eq!(
        delivery_of(Some(AttachmentDeliveryPolicy::NativeStructured)),
        DeliveryMode::NativeStructured
    );
}

/// §7.2's stages reach the orchestrator as the port's narrower set, and
/// every one of them arrives as something: the translations are exhaustive,
/// so a stage added to the context crate is a build error here.
#[test]
fn every_context_stage_reaches_the_orchestrator_as_one_of_its_own() {
    use koru_delegate_context::{CaptureStage, MaterializeStage};
    use koru_delegate_runtime::{CaptureStage as PortCapture, MaterializeStage as PortMaterialize};

    assert_eq!(capture_stage(CaptureStage::Resolve), PortCapture::Resolve);
    assert_eq!(capture_stage(CaptureStage::Capture), PortCapture::Commit);
    assert_eq!(
        capture_stage(CaptureStage::Retention),
        PortCapture::Retention
    );

    // Laying the directory out and writing a file are both writing; reading
    // the pinned bytes and rendering them are both reading what the pin
    // points at.
    assert_eq!(
        materialize_stage(MaterializeStage::Layout),
        PortMaterialize::Write
    );
    assert_eq!(
        materialize_stage(MaterializeStage::Write),
        PortMaterialize::Write
    );
    assert_eq!(
        materialize_stage(MaterializeStage::Read),
        PortMaterialize::Read
    );
    assert_eq!(
        materialize_stage(MaterializeStage::Render),
        PortMaterialize::Read
    );
}
