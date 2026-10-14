// SPDX-License-Identifier: MPL-2.0

//! Saved Cloud policy and transient permit progress are different facts.
//! A permit rotates out between passes; its absence must never switch off a
//! configured Workspace or discard its independently retained completion proof.
use koru_store_core::transport::EffectiveTransportMode;
use koru_store_wire::internal::CloudSyncState;
use koru_sync_control::cloud::CloudStage;

pub(super) fn state(
    policy: Option<EffectiveTransportMode>,
    runtime: Option<(CloudStage, Option<String>)>,
) -> Option<CloudSyncState> {
    let policy = policy?; // Unknown policy must not be reported as Off.
    let enabled = policy.stored_mode.min(policy.profile_mode).allows_cloud();
    let (stage, error_code) = if enabled && (policy.fenced || policy.ceiling_mismatch) {
        (
            Some(CloudStage::Unavailable),
            Some("mode_refuses_transport".to_owned()),
        )
    } else if enabled {
        runtime.map_or((None, None), |(stage, code)| (Some(stage), code))
    } else {
        (None, None)
    };
    Some(CloudSyncState {
        enabled,
        stage: stage
            .and_then(|stage| serde_json::to_value(stage).ok())
            .and_then(|value| value.as_str().map(str::to_owned)),
        error_code,
    })
}

#[cfg(test)]
mod tests;
