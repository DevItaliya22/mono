// SPDX-License-Identifier: MPL-2.0

use super::*;
use koru_store_core::{
    WorkspaceId,
    transport::{TransportMode, TransportSubject},
};

fn policy() -> EffectiveTransportMode {
    EffectiveTransportMode {
        subject: TransportSubject::Workspace(WorkspaceId::new()),
        mode: TransportMode::Cloud,
        profile_mode: TransportMode::Cloud,
        stored_mode: TransportMode::Cloud,
        explicit_choice: true,
        revision: 2,
        ceiling_mismatch: false,
        fenced: false,
        generation: 1,
    }
}

#[test]
fn retiring_a_permit_does_not_turn_off_cloud() {
    let live = state(Some(policy()), Some((CloudStage::Settled, None))).unwrap();
    let retired = state(Some(policy()), None).unwrap();
    assert!(live.enabled && retired.enabled);
    assert_eq!(live.stage.as_deref(), Some("settled"));
    assert_eq!(retired.stage, None);
    assert_eq!(retired.error_code, None);
}

#[test]
fn saved_off_overrides_a_stale_live_permit_and_profile_caps_workspace() {
    for profile_off in [false, true] {
        let mut policy = policy();
        if profile_off {
            policy.profile_mode = TransportMode::LocalOnly;
        } else {
            policy.stored_mode = TransportMode::LocalOnly;
        }
        let status = state(Some(policy), Some((CloudStage::Settled, None))).unwrap();
        assert!(!status.enabled);
        assert_eq!(status.stage, None);
    }
}

#[test]
fn a_fence_blocks_completion_without_rewriting_the_saved_choice() {
    let mut policy = policy();
    policy.fenced = true;
    policy.mode = TransportMode::LocalOnly;
    let status = state(Some(policy), Some((CloudStage::Settled, None))).unwrap();
    assert!(status.enabled);
    assert_eq!(status.stage.as_deref(), Some("unavailable"));
    assert_eq!(status.error_code.as_deref(), Some("mode_refuses_transport"));
}

#[test]
fn unknown_policy_is_not_an_off_claim() {
    assert!(state(None, Some((CloudStage::Settled, None))).is_none());
}
