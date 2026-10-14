// SPDX-License-Identifier: MPL-2.0

use super::*;

#[test]
fn actual_cloud_bucket_uses_frozen_digest_and_independent_compiled_policy() {
    let vectors: serde_json::Value = serde_json::from_str(include_str!(
        "../../../../contracts/vectors/cloud-sync-issuance-v1/issuance.json"
    ))
    .unwrap();
    let rate = &vectors["rate"];
    let identity = rate["identity"].as_str().unwrap().parse().unwrap();
    let device = DeviceId::from_uuid(rate["device"].as_str().unwrap().parse().unwrap());
    let workspace = WorkspaceId::from_uuid(rate["workspace"].as_str().unwrap().parse().unwrap());
    let harness = Harness::new();
    let crypto = AeadTransientEncryption::new(
        OpaqueSecret(vec![1; 32]),
        OpaqueSecret(vec![0x33; 32]),
        FakeEntropy(Mutex::new(0)),
    )
    .unwrap();
    let mut security = harness.service();
    security.crypto = &crypto;
    for remaining in (0..30).rev() {
        let decision =
            pollster::block_on(security.rate_decision(RateLimitSubject::CloudSyncToken {
                identity,
                device,
                workspace,
            }))
            .unwrap();
        assert!(decision.allowed);
        assert_eq!(decision.remaining, remaining);
    }
    let decision = pollster::block_on(security.rate_decision(RateLimitSubject::CloudSyncToken {
        identity,
        device,
        workspace,
    }))
    .unwrap();
    assert!(!decision.allowed);
    assert_eq!(decision.retry_after_millis, Some(60_000));
    let digest: String = harness
        .limits
        .0
        .lock()
        .unwrap()
        .keys()
        .next()
        .unwrap()
        .iter()
        .map(|b| format!("{b:02x}"))
        .collect();
    assert_eq!(digest, rate["golden_hmac_hex"].as_str().unwrap(), "I-B01");
    assert_eq!(
        CLOUD_SYNC_TOKEN_RATE,
        RatePolicy {
            scope: RateLimitScope::DeviceOperation,
            limit: 30,
            window_millis: 60_000
        },
        "I-B03"
    );
    let another_identity = "aaaaaaaa-aaaa-4aaa-8aaa-aaaaaaaaaaaa".parse().unwrap();
    let another_device =
        DeviceId::from_uuid("aaaaaaaa-aaaa-4aaa-8aaa-aaaaaaaaaaaa".parse().unwrap());
    let another_workspace =
        WorkspaceId::from_uuid("aaaaaaaa-aaaa-4aaa-8aaa-aaaaaaaaaaaa".parse().unwrap());
    for subject in [
        RateLimitSubject::CloudSyncToken {
            identity: another_identity,
            device,
            workspace,
        },
        RateLimitSubject::CloudSyncToken {
            identity,
            device: another_device,
            workspace,
        },
        RateLimitSubject::CloudSyncToken {
            identity,
            device,
            workspace: another_workspace,
        },
        RateLimitSubject::DeviceOperation { identity, device },
        RateLimitSubject::Enrollment {
            identity,
            device,
            operation: EnrollmentRateOperation::Discovery,
        },
        RateLimitSubject::Enrollment {
            identity,
            device,
            operation: EnrollmentRateOperation::Attestation,
        },
        RateLimitSubject::Enrollment {
            identity,
            device,
            operation: EnrollmentRateOperation::Provisioning,
        },
    ] {
        assert!(
            pollster::block_on(security.rate_decision(subject))
                .unwrap()
                .allowed,
            "independent digest/allowance"
        );
    }
    assert_eq!(harness.limits.0.lock().unwrap().len(), 8);
    harness.clock.0.fetch_add(59_999, Ordering::SeqCst);
    assert!(
        !pollster::block_on(security.rate_decision(RateLimitSubject::CloudSyncToken {
            identity,
            device,
            workspace
        }))
        .unwrap()
        .allowed
    );
    harness.clock.0.fetch_add(1, Ordering::SeqCst);
    assert!(
        pollster::block_on(security.rate_decision(RateLimitSubject::CloudSyncToken {
            identity,
            device,
            workspace
        }))
        .unwrap()
        .allowed
    );
    // The real policy/encoding/crypto ran; counting/window storage is the named
    // in-memory Limits port, not evidence for PostgreSQL concurrency.
}
