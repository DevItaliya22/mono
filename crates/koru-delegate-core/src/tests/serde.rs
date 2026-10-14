// SPDX-License-Identifier: MPL-2.0

//! The sync shapes round-trip, read what an older writer wrote, and read
//! what a newer writer will write (§8: an older reader preserves and
//! ignores what it does not know — here, ignores; the Run document
//! preserves).

use super::*;

#[test]
fn a_whole_run_round_trips_through_json() {
    let run = DelegatedRun {
        header: header(),
        timeline: vec![
            entry(StepKind::RunCreated, 1_000),
            TimelineEntry::new(
                StepKind::TurnStarted {
                    originator: TurnOriginator::Orchestrator,
                },
                TimelineRefs::new([TimelineRef::Turn { turn: turn_id(2) }]).unwrap(),
                2_000,
            ),
            entry(
                StepKind::Gap {
                    events_lost: Some(4),
                },
                3_000,
            ),
        ],
    };
    let json = serde_json::to_string(&run).unwrap();
    let back: DelegatedRun = serde_json::from_str(&json).unwrap();
    assert_eq!(back, run);
}

#[test]
fn a_header_from_an_older_writer_reads_with_defaults() {
    // No grants, artifacts, economics or rating; a running status with no
    // markers spelled out.
    let older = serde_json::json!({
        "run": run_id(1).to_storage(),
        "created_at_ms": 1000,
        "requested_by": { "kind": "person", "profile": v4(2).to_string() },
        "objective": "older",
        "agent": { "kind": "built_in", "value": "codex" },
        "scope": { "kind": "temporary" },
        "context": [],
        "status": { "state": "running" },
        "environment": { "kind": "local_device", "device": v4(9).to_string() },
    });
    let header: RunHeader = serde_json::from_value(older).unwrap();
    assert!(header.grants.is_empty());
    assert!(header.artifacts.is_empty());
    assert_eq!(header.economics, EconomicsSummary::default());
    assert_eq!(header.rating, None);
    assert_eq!(header.status, RunStatus::running());
    assert_eq!(header.scope, WorkspaceScope::Temporary);
}

#[test]
fn a_header_from_a_newer_writer_reads_past_what_it_does_not_know() {
    let mut newer = serde_json::to_value(header()).unwrap();
    newer["a_field_from_m4"] = serde_json::json!({ "worktree": "abc" });
    newer["status"]["a_marker_from_m5"] = serde_json::json!(true);
    newer["artifacts"][0]["a_field_from_m3"] = serde_json::json!(7);
    let back: RunHeader = serde_json::from_value(newer).unwrap();
    assert_eq!(back, header());
}

#[test]
fn a_status_serializes_under_a_state_tag_with_its_markers() {
    let status = RunStatus::Running {
        attention: [Attention::ApprovalRequired, Attention::WaitingForInput]
            .into_iter()
            .collect(),
        control: ControlState::HandoverPending {
            objective_satisfied: true,
        },
    };
    let json = serde_json::to_value(&status).unwrap();
    assert_eq!(
        json,
        serde_json::json!({
            "state": "running",
            "attention": ["approval_required", "waiting_for_input"],
            "control": { "holder": "handover_pending", "objective_satisfied": true },
        })
    );
    assert_eq!(
        serde_json::to_value(RunStatus::Failed {
            stage: FailureStage::ContextCapture
        })
        .unwrap(),
        serde_json::json!({ "state": "failed", "stage": "context_capture" })
    );
}

#[test]
fn the_attempt_state_round_trips_and_defaults_its_optional_facts() {
    let state = AttemptState::first(run_id(1));
    let json = serde_json::to_value(&state).unwrap();
    assert_eq!(json["generation"], 1);
    assert!(json.get("provider_session").is_none());
    let back: AttemptState = serde_json::from_value(json).unwrap();
    assert_eq!(back, state);

    let minimal = serde_json::json!({
        "run": run_id(1).to_storage(),
        "attempt": attempt_id(2).to_storage(),
        "generation": 4,
    });
    let back: AttemptState = serde_json::from_value(minimal).unwrap();
    assert_eq!(back.generation.get(), 4);
    assert_eq!(back.process, ProcessFacts::default());
    assert!(back.pending_publications.is_empty());
}

#[test]
fn context_items_and_scopes_are_tagged_by_kind() {
    let request = request();
    let json = serde_json::to_value(&request).unwrap();
    assert_eq!(json["context"][0]["kind"], "document");
    assert_eq!(json["context"][1]["kind"], "block");
    assert_eq!(json["context"][2]["kind"], "blob");
    assert_eq!(json["context"][2]["blob_kind"], "pdf");
    assert_eq!(json["scope"]["kind"], "project_folder");
    assert_eq!(json["executing_device"], v4(9).to_string());
    let back: DelegationRequest = serde_json::from_value(json).unwrap();
    assert_eq!(back, request);
}

#[test]
fn a_header_is_made_from_a_request_and_starts_created() {
    let request = request();
    let header = RunHeader::from_request(request.clone(), 1_234);
    assert_eq!(header.run, request.run);
    assert_eq!(header.status, RunStatus::Created);
    assert_eq!(header.created_at_ms, 1_234);
    assert_eq!(header.executing_device(), Some(request.executing_device));
    assert_eq!(header.grants, request.grants);
    assert_eq!(header.context, request.context);
    assert!(header.artifacts.is_empty());
    assert_eq!(header.rating, None);
    assert_eq!(header.scope.project(), request.scope.project());
    assert!(header.scope.is_admissible());
    assert!(!WorkspaceScope::None.is_admissible());
}

#[test]
fn model_choice_is_optional_retained_intent_and_redacted_in_debug() {
    let original = request();
    let old = serde_json::to_value(&original).unwrap();
    assert!(old.get("requested_model").is_none());
    let decoded: DelegationRequest = serde_json::from_value(old).unwrap();
    assert_eq!(decoded.requested_model, None);

    let mut chosen = original;
    chosen.requested_model = Some(ModelName::new("private-provider/MODEL-SENTINEL").unwrap());
    let encoded = serde_json::to_value(&chosen).unwrap();
    assert_eq!(
        encoded["requested_model"],
        "private-provider/MODEL-SENTINEL"
    );
    assert_eq!(
        serde_json::from_value::<DelegationRequest>(encoded).unwrap(),
        chosen
    );
    let header = RunHeader::from_request(chosen.clone(), 1000);
    assert_eq!(header.requested_model, chosen.requested_model);
    assert!(!format!("{chosen:?} {header:?}").contains("MODEL-SENTINEL"));
    let mut old_header = serde_json::to_value(&header).unwrap();
    old_header
        .as_object_mut()
        .unwrap()
        .remove("requested_model");
    assert_eq!(
        serde_json::from_value::<RunHeader>(old_header)
            .unwrap()
            .requested_model,
        None
    );
}

#[test]
fn failure_reason_defaults_and_normalizes_without_retaining_raw_errors() {
    let old = serde_json::to_value(header()).unwrap();
    assert!(old.get("failure_reason").is_none());
    let decoded: RunHeader = serde_json::from_value(old).unwrap();
    assert_eq!(decoded.failure_reason, None);
    let step: StepKind =
        serde_json::from_value(serde_json::json!({"step":"failed", "stage":"session_creation"}))
            .unwrap();
    assert!(matches!(step, StepKind::Failed { reason: None, .. }));
    let reason = RunFailureReason::from_code("SECRET raw provider error");
    assert_eq!(reason, RunFailureReason::Unknown);
    assert_eq!(serde_json::to_value(reason).unwrap(), "unknown");
    assert_eq!(
        serde_json::from_value::<RunFailureReason>(serde_json::json!("future_reason")).unwrap(),
        RunFailureReason::Unknown
    );
}
