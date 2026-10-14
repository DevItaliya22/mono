// SPDX-License-Identifier: MPL-2.0

use super::*;
use ed25519_dalek::SigningKey;
use koru_account_core::{AccountSessionId, KoruIdentityId};
use koru_account_session::{
    DEVICE_ATTESTATION_AUDIENCE, DEVICE_ATTESTATION_VERSION, DeviceAttestationClaimsV1,
    DeviceAttestationKeyRing, verify_device_attestation,
};
use koru_device::{DeviceSigningKey, ReplicationEndpointSecret};
use std::str::FromStr as _;

fn device(name: &str) -> (DeviceSigningKey, DeviceDescriptorV1) {
    let key = DeviceSigningKey::generate();
    let endpoint = ReplicationEndpointSecret::generate();
    let descriptor = DeviceDescriptorV1::new(
        DeviceId::new(),
        name.into(),
        &key,
        endpoint.endpoint_id(),
        1,
    );
    (key, descriptor)
}

fn profile() -> ProfileId {
    ProfileId::parse("60000000-0000-4000-8000-000000000006").unwrap()
}

fn attestation(
    joining: &DeviceDescriptorV1,
    authorized_profile_ids: Vec<ProfileId>,
) -> (SignedDeviceAttestationV1, VerifiedDeviceAttestationV1) {
    let ring = DeviceAttestationKeyRing::new(
        "attestation".into(),
        [("attestation".into(), SigningKey::from_bytes(&[41; 32]))],
    )
    .unwrap();
    let signed = ring
        .sign(&DeviceAttestationClaimsV1 {
            version: DEVICE_ATTESTATION_VERSION,
            issuer: "https://api.koru.do".into(),
            audience: DEVICE_ATTESTATION_AUDIENCE.into(),
            koru_identity_id: KoruIdentityId::from_str("10000000-0000-4000-8000-000000000001")
                .unwrap(),
            device_id: joining.device_id,
            signing_public_key: joining.signing_public_key,
            endpoint_id: joining.endpoint_id,
            authorized_profile_ids,
            session_id: AccountSessionId::from_str("30000000-0000-4000-8000-000000000003").unwrap(),
            token_id: "enrollment-jti".into(),
            issued_at_millis: 1_000_000,
            expires_at_millis: 1_120_000,
        })
        .unwrap();
    let verified = verify_device_attestation(
        &signed,
        &ring.verification_keys(),
        "https://api.koru.do",
        1_001_000,
    )
    .unwrap();
    (signed, verified)
}

fn prepared_enrollment() -> (
    DeviceSigningKey,
    DeviceSigningKey,
    VerifiedDeviceAttestationV1,
    PreparedEnrollmentV1,
) {
    let (existing_key, existing) = device("Existing");
    let (joining_key, joining) = device("Joining");
    let (signed, verified) = attestation(&joining, vec![profile()]);
    let transcript =
        EnrollmentTranscriptV1::new(&verified, profile(), existing, joining, [51; 32]).unwrap();
    let join =
        EnrollmentJoinV1::sign(signed, &verified, transcript, &joining_key, 1_002_000).unwrap();
    let prepared = join.complete(&verified, &existing_key).unwrap();
    (existing_key, joining_key, verified, prepared)
}

#[test]
fn invitation_qr_round_trips_and_binds_endpoint() {
    let (_, inviter) = device("Windows");
    let invitation = PairingInvitationV1::new(inviter, Some("https://relay.example".into()));
    let decoded = PairingInvitationV1::decode_qr(&invitation.encode_qr().unwrap()).unwrap();
    assert_eq!(decoded, invitation);
}

#[test]
fn invitation_link_is_clickable_and_legacy_qr_values_still_work() {
    let (_, inviter) = device("Android");
    let invitation = PairingInvitationV1::new(inviter, None);
    let link = invitation.encode_link_url().unwrap();
    assert!(link.starts_with(PAIRING_LINK_PREFIX));
    assert_eq!(PairingInvitationV1::decode_link(&link).unwrap(), invitation);
    let compact = invitation.encode_qr().unwrap();
    assert!(compact.starts_with("koru-pair:v1c:"));
    assert!(
        compact.len() < 700,
        "compact invitation was {} bytes",
        compact.len()
    );
    assert_eq!(
        PairingInvitationV1::decode_link(&compact).unwrap(),
        invitation
    );

    let legacy = format!(
        "koru-pair:v1:{}",
        base64::engine::general_purpose::URL_SAFE_NO_PAD
            .encode(serde_json::to_vec(&invitation).unwrap())
    );
    assert_eq!(
        PairingInvitationV1::decode_link(&legacy).unwrap(),
        invitation
    );
}

#[test]
fn invitation_is_one_use_and_monotonic_expiry_wins() {
    let (_, inviter) = device("Windows");
    let now = Instant::now();
    let mut session = InvitationSession::new(inviter, None, now);
    let secret = session.invitation.secret.clone();
    session.claim(&secret, DeviceId::new(), now).unwrap();
    assert_eq!(
        session.claim(&secret, DeviceId::new(), now),
        Err(PairingError::AlreadyClaimed)
    );
    let (_, inviter) = device("Windows");
    let mut expired = InvitationSession::new(inviter, None, now);
    assert_eq!(
        expired.claim(
            &expired.invitation.secret.clone(),
            DeviceId::new(),
            now + INVITATION_LIFETIME
        ),
        Err(PairingError::Expired)
    );
}

#[test]
fn both_device_proofs_are_bound_to_every_transcript_field() {
    let (inviter_key, inviter) = device("Windows");
    let (joiner_key, joiner) = device("Phone");
    let invitation = PairingInvitationV1::new(inviter.clone(), None);
    let transcript = PairingTranscriptV1::new(&invitation, joiner.clone());
    let inviter_proof = transcript.sign(inviter.device_id, &inviter_key).unwrap();
    let joiner_proof = transcript.sign(joiner.device_id, &joiner_key).unwrap();
    transcript.verify_proof(&inviter_proof).unwrap();
    transcript.verify_proof(&joiner_proof).unwrap();
    let mut tampered = transcript;
    tampered.joiner_challenge[0] ^= 1;
    assert_eq!(
        tampered.verify_proof(&inviter_proof),
        Err(PairingError::InvalidSignature)
    );
}

#[test]
fn pairing_secrets_never_appear_in_debug() {
    assert_eq!(
        format!("{:?}", PairingSecret::generate()),
        "PairingSecret([REDACTED])"
    );
}

#[test]
fn prepared_pairing_binds_relationship_and_bilateral_grants() {
    let (inviter_key, inviter) = device("Windows");
    let (joiner_key, joiner) = device("Phone");
    let invitation = PairingInvitationV1::new(inviter, None);
    let transcript = PairingTranscriptV1::new(&invitation, joiner);
    let prepared =
        PreparedPairingV1::sign_for_test(transcript, &inviter_key, &joiner_key, 42).unwrap();
    prepared.verify().unwrap();

    let mut changed = prepared.clone();
    changed.inviter_grant.device_id = changed.transcript.inviter.device_id;
    assert_eq!(changed.verify(), Err(PairingError::TranscriptMismatch));
}

#[test]
fn production_exchange_combines_independently_signed_halves() {
    let (inviter_key, inviter) = device("Windows");
    let (joiner_key, joiner) = device("Android");
    let invitation = PairingInvitationV1::new(inviter, None);
    let join = PairingJoinV1::sign(&invitation, joiner, &joiner_key, 42).unwrap();

    let prepared = join.complete(&invitation, &inviter_key).unwrap();
    prepared.verify().unwrap();
    assert_eq!(
        prepared.joiner_grant.issued_by,
        prepared.transcript.joiner.device_id
    );
    assert_eq!(
        prepared.inviter_grant.issued_by,
        prepared.transcript.inviter.device_id
    );
}

#[test]
fn a_join_half_cannot_be_replayed_against_another_invitation() {
    let (_, inviter) = device("Windows");
    let (joiner_key, joiner) = device("Android");
    let invitation = PairingInvitationV1::new(inviter.clone(), None);
    let join = PairingJoinV1::sign(&invitation, joiner, &joiner_key, 42).unwrap();
    let another = PairingInvitationV1::new(inviter, None);

    assert_eq!(join.verify(&another), Err(PairingError::WrongSecret));
}

#[test]
fn enrollment_binds_attestation_profile_descriptors_challenges_and_grants() {
    let (_, _, verified, prepared) = prepared_enrollment();
    prepared.verify(&verified).unwrap();

    let mut changed = prepared.clone();
    changed.transcript.existing_challenge[0] ^= 1;
    assert_eq!(
        changed.verify(&verified),
        Err(PairingError::InvalidSignature)
    );
    let mut changed = prepared.clone();
    changed.transcript.joining_challenge[0] ^= 1;
    assert_eq!(
        changed.verify(&verified),
        Err(PairingError::InvalidSignature)
    );
    let mut changed = prepared.clone();
    changed.transcript.relationship_id = RelationshipId::new();
    assert_eq!(
        changed.verify(&verified),
        Err(PairingError::InvalidSignature)
    );
    let mut changed = prepared.clone();
    changed.transcript.existing_device.display_name.push('x');
    assert_eq!(
        changed.verify(&verified),
        Err(PairingError::InvalidSignature)
    );
    let mut changed = prepared.clone();
    changed.transcript.joining_device.display_name.push('x');
    assert_eq!(
        changed.verify(&verified),
        Err(PairingError::InvalidSignature)
    );
    let mut changed = prepared.clone();
    changed.transcript.attestation_jti.push('x');
    assert_eq!(
        changed.verify(&verified),
        Err(PairingError::TranscriptMismatch)
    );
    let mut changed = prepared.clone();
    changed.transcript.attestation_digest[0] ^= 1;
    assert_eq!(
        changed.verify(&verified),
        Err(PairingError::TranscriptMismatch)
    );
    let mut changed = prepared.clone();
    changed.transcript.profile_id = ProfileId::new();
    assert_eq!(
        changed.verify(&verified),
        Err(PairingError::TranscriptMismatch)
    );
    let mut changed = prepared.clone();
    changed.existing_grant.profile_id = ProfileId::new();
    assert_eq!(
        changed.verify(&verified),
        Err(PairingError::TranscriptMismatch)
    );
}

#[test]
fn enrollment_rejects_attested_device_key_and_endpoint_substitution() {
    let (_, _, verified, prepared) = prepared_enrollment();
    let mut wrong_descriptor = prepared.transcript.joining_device.clone();
    wrong_descriptor.device_id = DeviceId::new();
    let (_, wrong) = attestation(&wrong_descriptor, vec![profile()]);
    assert_eq!(
        prepared.verify(&wrong),
        Err(PairingError::TranscriptMismatch)
    );
    let mut wrong_descriptor = prepared.transcript.joining_device.clone();
    wrong_descriptor.signing_public_key = DeviceSigningKey::generate().public_key();
    let (_, wrong) = attestation(&wrong_descriptor, vec![profile()]);
    assert_eq!(
        prepared.verify(&wrong),
        Err(PairingError::TranscriptMismatch)
    );
    let mut wrong_descriptor = prepared.transcript.joining_device.clone();
    wrong_descriptor.endpoint_id = ReplicationEndpointSecret::generate().endpoint_id();
    let (_, wrong) = attestation(&wrong_descriptor, vec![profile()]);
    assert_eq!(
        prepared.verify(&wrong),
        Err(PairingError::TranscriptMismatch)
    );
    for mutate in [
        |descriptor: &mut DeviceDescriptorV1| descriptor.device_id = DeviceId::new(),
        |descriptor: &mut DeviceDescriptorV1| {
            descriptor.signing_public_key = DeviceSigningKey::generate().public_key()
        },
        |descriptor: &mut DeviceDescriptorV1| {
            descriptor.endpoint_id = ReplicationEndpointSecret::generate().endpoint_id()
        },
    ] as [fn(&mut DeviceDescriptorV1); 3]
    {
        let mut changed = prepared.clone();
        mutate(&mut changed.transcript.joining_device);
        assert!(changed.verify(&verified).is_err());
    }
    assert_eq!(
        verify_enrollment_responder_transport(
            &prepared.transcript,
            prepared.transcript.existing_device.endpoint_id,
            ReplicationEndpointSecret::generate().endpoint_id(),
        ),
        Err(PairingError::EndpointMismatch)
    );
    verify_enrollment_joiner_transport(
        &prepared.transcript,
        prepared.transcript.joining_device.endpoint_id,
        prepared.transcript.existing_device.endpoint_id,
    )
    .unwrap();
}

#[test]
fn enrollment_rejects_an_attestation_for_another_profile() {
    let (_, existing) = device("Existing");
    let (_, joining) = device("Joining");
    let (_, verified) = attestation(&joining, vec![ProfileId::new()]);
    assert_eq!(
        EnrollmentTranscriptV1::new(&verified, profile(), existing, joining, [1; 32]),
        Err(PairingError::TranscriptMismatch)
    );
}

#[test]
fn enrollment_and_pairing_transcripts_are_domain_separated() {
    let (inviter_key, inviter) = device("Existing");
    let (_, joining) = device("Joining");
    let invitation = PairingInvitationV1::new(inviter, None);
    let pairing = PairingTranscriptV1::new(&invitation, joining.clone());
    let pairing_proof = pairing
        .sign(pairing.inviter.device_id, &inviter_key)
        .unwrap();
    let (_, verified) = attestation(&joining, vec![profile()]);
    let enrollment = EnrollmentTranscriptV1::new(
        &verified,
        profile(),
        pairing.inviter.clone(),
        joining,
        pairing.joiner_challenge,
    )
    .unwrap();
    let substituted = EnrollmentProofV1 {
        signer: pairing_proof.signer,
        signature: pairing_proof.signature,
    };
    assert_eq!(
        enrollment.verify_proof(&substituted),
        Err(PairingError::InvalidSignature)
    );
}

#[test]
fn enrollment_receipt_codec_is_canonical_bounded_and_redacted() {
    let (_, _, _, prepared) = prepared_enrollment();
    let encoded = prepared.encode_receipt().unwrap();
    assert_eq!(
        PreparedEnrollmentV1::decode_receipt(&encoded).unwrap(),
        prepared
    );
    assert_eq!(prepared.encode_receipt().unwrap(), encoded);
    assert!(encoded.len() < MAX_ENROLLMENT_RECEIPT_BYTES * 4 / 3 + 4);

    assert_eq!(
        PreparedEnrollmentV1::decode_receipt(&(encoded.clone() + "=")),
        Err(PairingError::Malformed)
    );
    let mut bytes = base64::engine::general_purpose::URL_SAFE_NO_PAD
        .decode(&encoded)
        .unwrap();
    bytes.push(0);
    let trailing = base64::engine::general_purpose::URL_SAFE_NO_PAD.encode(bytes);
    assert_eq!(
        PreparedEnrollmentV1::decode_receipt(&trailing),
        Err(PairingError::Malformed)
    );
    assert_eq!(
        PreparedEnrollmentV1::decode_receipt(&"A".repeat(MAX_ENROLLMENT_RECEIPT_BYTES * 4 / 3 + 5)),
        Err(PairingError::Malformed)
    );

    let debug = format!(
        "{prepared:?} {:?} {:?} {:?}",
        prepared.transcript, prepared.existing_proof, prepared.joining_proof
    );
    for secret in [
        &prepared.attestation.compact,
        &prepared.transcript.attestation_jti,
        &prepared.transcript.profile_id.to_string(),
        &prepared.transcript.existing_device.device_id.to_string(),
        &prepared.transcript.joining_device.device_id.to_string(),
    ] {
        assert!(!debug.contains(secret));
    }
}

#[test]
fn legacy_qr_transcript_and_all_data_records_match_the_frozen_vector() {
    let inviter_key = DeviceSigningKey::from_secret_bytes([1; 32]);
    let joiner_key = DeviceSigningKey::from_secret_bytes([2; 32]);
    let inviter = DeviceDescriptorV1::new(
        DeviceId::parse("10000000-0000-4000-8000-000000000001").unwrap(),
        "Windows".into(),
        &inviter_key,
        ReplicationEndpointSecret::from_secret_bytes([3; 32]).endpoint_id(),
        1_000,
    );
    let joiner = DeviceDescriptorV1::new(
        DeviceId::parse("20000000-0000-4000-8000-000000000002").unwrap(),
        "Android".into(),
        &joiner_key,
        ReplicationEndpointSecret::from_secret_bytes([4; 32]).endpoint_id(),
        2_000,
    );
    let invitation = PairingInvitationV1 {
        version: PAIRING_VERSION,
        invitation_id: InvitationId(
            uuid::Uuid::parse_str("30000000-0000-4000-8000-000000000003").unwrap(),
        ),
        inviter: inviter.clone(),
        pairing_target: IrohPairingTarget {
            endpoint_id: inviter.endpoint_id,
            relay_hint: None,
        },
        secret: PairingSecret([5; 32]),
        expires_in_seconds: 120,
    };
    let transcript = PairingTranscriptV1 {
        version: PAIRING_VERSION,
        invitation_id: invitation.invitation_id,
        relationship_id: RelationshipId::from_uuid(
            uuid::Uuid::parse_str("40000000-0000-4000-8000-000000000004").unwrap(),
        ),
        inviter,
        joiner,
        grant_scope: GrantScopeV1::AllData,
        inviter_challenge: [6; 32],
        joiner_challenge: [7; 32],
    };
    let inviter_proof = transcript
        .sign(transcript.inviter.device_id, &inviter_key)
        .unwrap();
    let joiner_proof = transcript
        .sign(transcript.joiner.device_id, &joiner_key)
        .unwrap();
    let draft = relationship_draft(&transcript, 3_000);
    let relationship = draft.clone().with_signatures(
        inviter_key.sign_message(&draft.signing_bytes()),
        joiner_key.sign_message(&draft.signing_bytes()),
    );
    let inviter_grant = AllDataGrantV1::sign(
        transcript.relationship_id,
        transcript.joiner.device_id,
        transcript.inviter.device_id,
        RecordId::from_uuid(uuid::Uuid::parse_str("50000000-0000-4000-8000-000000000005").unwrap()),
        3_000,
        &inviter_key,
    );
    let joiner_grant = AllDataGrantV1::sign(
        transcript.relationship_id,
        transcript.inviter.device_id,
        transcript.joiner.device_id,
        RecordId::from_uuid(uuid::Uuid::parse_str("60000000-0000-4000-8000-000000000006").unwrap()),
        3_000,
        &joiner_key,
    );
    let prepared = PreparedPairingV1::assemble(
        transcript,
        inviter_proof,
        joiner_proof,
        relationship,
        inviter_grant,
        joiner_grant,
    )
    .unwrap();
    assert_eq!(
        invitation.encode_qr().unwrap(),
        "koru-pair:v1c:ARAwAAAAAABAAIAAAAAAAAADARAQAAAAAABAAIAAAAAAAAABB1dpbmRvd3OKiOPddAnxlf1S2y08ul1yymcJvx2UEhvzdIgBtA9vXO1JKMYo0cLG6ukDOJBZlWEpWSc6XGP5NjbBRhSshzfR0A9AVCsWUlSYhDazVsXS10N7s6QWXJubXxJSAWsiAEUFaH2pYQsKYfGa-vtXNAkLNl_VGJlEQGh1BY6lB5a_-U0BCe1JKMYo0cLG6ukDOJBZlWEpWSc6XGP5NjbBRhSshzfRACAFBQUFBQUFBQUFBQUFBQUFBQUFBQUFBQUFBQUFBQUFBXg"
    );
    assert_eq!(
        hex(&transcript_hash(&prepared.transcript)),
        "b90de21d030ff36dfe14b603cca3f5ed59659c0998e0a2d1e652d9e8730ba579"
    );
    assert_eq!(
        serde_json::to_string(&prepared.inviter_grant).unwrap(),
        r#"{"version":1,"relationship_id":"40000000-0000-4000-8000-000000000004","device_id":"20000000-0000-4000-8000-000000000002","issued_by":"10000000-0000-4000-8000-000000000001","record_id":"50000000-0000-4000-8000-000000000005","issued_at_millis":3000,"signature":[165,43,128,211,253,104,23,160,53,78,1,109,193,235,47,98,143,233,173,2,37,198,204,161,82,154,184,183,142,184,50,100,121,35,226,54,43,26,59,217,61,131,165,151,104,128,39,238,120,77,160,142,160,91,52,46,157,154,102,243,35,56,139,15]}"#
    );
    assert_eq!(
        serde_json::to_string(&prepared.joiner_grant).unwrap(),
        r#"{"version":1,"relationship_id":"40000000-0000-4000-8000-000000000004","device_id":"10000000-0000-4000-8000-000000000001","issued_by":"20000000-0000-4000-8000-000000000002","record_id":"60000000-0000-4000-8000-000000000006","issued_at_millis":3000,"signature":[105,218,122,130,110,217,177,46,72,168,58,46,33,11,0,70,145,188,230,89,145,145,65,21,79,102,154,0,5,147,50,149,179,29,146,28,207,45,147,66,207,151,169,196,100,47,72,45,248,214,248,109,166,255,225,194,85,221,174,161,24,27,116,9]}"#
    );
}

fn hex(bytes: &[u8]) -> String {
    bytes.iter().map(|byte| format!("{byte:02x}")).collect()
}
