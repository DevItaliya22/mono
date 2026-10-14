// SPDX-License-Identifier: MPL-2.0

use super::*;

#[test]
fn account_open_failure_never_returns_private_storage_detail() {
    assert_eq!(
        account_host_error("permission denied at private-person/store.db".into()),
        "account_host_unavailable"
    );
    assert_eq!(
        account_host_error("invalid_device_seeds".into()),
        "invalid_device_seeds"
    );
    let result = account_poll(wire::AccountPollRequest {
        version: 1,
        requester: 1,
        ..Default::default()
    });
    assert_eq!(result.error_code.as_deref(), Some("missing_host_context"));
}

#[test]
fn encoding_cleanup_removes_nonce_and_vault_copies() {
    let mut reply = wire::AccountPollReply {
        sign_in: Some(wire::NativeSignInPrompt {
            nonce: "private".into(),
            ..Default::default()
        }),
        vault: Some(wire::AccountVaultPrompt {
            credential: Some(b"private".to_vec()),
            ..Default::default()
        }),
        ..Default::default()
    };
    zeroize_account_poll(&mut reply);
    assert!(reply.sign_in.unwrap().nonce.is_empty());
    assert!(reply.vault.unwrap().credential.unwrap().is_empty());
}

#[test]
fn malformed_trailing_fields_fail_with_a_closed_decode_error() {
    use prost::Message;
    let request = wire::AccountPlatformRequest {
        version: 1,
        requester: 1,
        response: Some(wire::account_platform_request::Response::SignIn(
            wire::NativeSignInReply {
                generation: 1,
                request_id: 1,
                id_token: Some(b"private-token".to_vec()),
                cancelled: false,
            },
        )),
        ..Default::default()
    };
    let mut bytes = request.encode_to_vec();
    bytes.push(255);
    assert_eq!(
        decode_account_platform(&bytes).err(),
        Some("invalid_protobuf")
    );
}
