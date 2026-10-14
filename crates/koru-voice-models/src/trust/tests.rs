// SPDX-License-Identifier: MPL-2.0

use super::*;
use crate::descriptor::{DESCRIPTOR_SCHEMA_VERSION, Sha256Digest};
use crate::fixtures::{self, Signer};

#[test]
fn a_signature_from_a_pinned_key_verifies() {
    let signer = Signer::new();
    let descriptor = fixtures::descriptor();
    let signature = signer.sign(&descriptor);
    signer.trusted.verify(&descriptor, &signature).unwrap();
}

/// **The vector the whole ADR is about.**
///
/// A *real* signature, made by a key we pin, over a descriptor naming a
/// different digest — then presented beside the descriptor the attacker wants
/// installed. A digest check alone sees nothing wrong with either half; only
/// signing the whole descriptor catches it, and only because the bytes the
/// signature was made over are not the bytes we canonicalise now.
#[test]
fn a_valid_signature_over_the_wrong_digest_is_refused() {
    let signer = Signer::new();
    let published = fixtures::descriptor();
    let substituted = ModelDescriptor {
        digest: Sha256Digest::parse(
            "0000000000000000000000000000000000000000000000000000000000000000",
        )
        .unwrap(),
        ..published.clone()
    };
    let lifted = signer.sign(&published);
    let error = signer.trusted.verify(&substituted, &lifted).unwrap_err();
    assert_eq!(error.code, ModelErrorCode::SignatureInvalid);
}

/// The same trick with `engine` instead of `digest`, because that is the field
/// whose forgery routes verified bytes into the wrong native parser and is
/// therefore the one a reviewer is most likely to be talked into leaving out.
#[test]
fn a_valid_signature_over_a_different_engine_is_refused() {
    let signer = Signer::new();
    let published = fixtures::descriptor();
    let rerouted = ModelDescriptor {
        engine: koru_voice::EngineId::parse("whisper").unwrap(),
        ..published.clone()
    };
    let lifted = signer.sign(&published);
    assert_eq!(
        signer.trusted.verify(&rerouted, &lifted).unwrap_err().code,
        ModelErrorCode::SignatureInvalid
    );
}

/// *You are probably running an old build*, not *somebody changed something*.
/// Minisign compares key ids before it hashes anything, which is what makes the
/// two answerable separately.
#[test]
fn a_signature_from_a_key_nobody_pinned_says_so() {
    let ours = Signer::new();
    let theirs = Signer::new();
    let descriptor = fixtures::descriptor();
    let signature = theirs.sign(&descriptor);
    let error = ours.trusted.verify(&descriptor, &signature).unwrap_err();
    assert_eq!(error.code, ModelErrorCode::SignatureUnknownKey);
}

#[test]
fn a_signature_that_does_not_decode_is_its_own_code() {
    let signer = Signer::new();
    let descriptor = fixtures::descriptor();
    let rubbish = ModelSignature::parse("untrusted comment: nope\nnot base64 at all").unwrap();
    assert_eq!(
        signer
            .trusted
            .verify(&descriptor, &rubbish)
            .unwrap_err()
            .code,
        ModelErrorCode::SignatureMalformed
    );
    assert_eq!(
        ModelSignature::parse("   ").unwrap_err().code,
        ModelErrorCode::SignatureMalformed
    );
}

/// A build with no key refuses and says which of the two problems it has. The
/// alternative — checking the digest alone while a key is pending — is the
/// degradation ADR-250 exists to refuse, and it would be invisible.
#[test]
fn a_build_with_no_pinned_key_refuses_rather_than_falling_back() {
    let signer = Signer::new();
    let empty = TrustedKeys::from_public_keys::<&str>(&[]).unwrap();
    assert!(empty.is_empty());
    let descriptor = fixtures::descriptor();
    let error = empty
        .verify(&descriptor, &signer.sign(&descriptor))
        .unwrap_err();
    assert_eq!(error.code, ModelErrorCode::NoTrustRoot);
}

/// Rotation is additive: both keys verify while both are listed, so a release
/// can introduce a key before anything signs with it and retire the old one a
/// release later.
#[test]
fn two_pinned_keys_both_verify() {
    let old = Signer::new();
    let new = Signer::new();
    let both =
        TrustedKeys::from_public_keys(&[old.public_key_text(), new.public_key_text()]).unwrap();
    assert_eq!(both.len(), 2);
    let descriptor = fixtures::descriptor();
    both.verify(&descriptor, &old.sign(&descriptor)).unwrap();
    both.verify(&descriptor, &new.sign(&descriptor)).unwrap();
}

/// A descriptor whose schema this build does not know never reaches the
/// signature check, so an old build reports its age rather than an attack.
#[test]
fn an_unknown_schema_is_reported_before_the_signature_is_looked_at() {
    let signer = Signer::new();
    let future = ModelDescriptor {
        schema_version: DESCRIPTOR_SCHEMA_VERSION + 1,
        ..fixtures::descriptor()
    };
    let signature = ModelSignature::parse("anything at all").unwrap();
    assert_eq!(
        signer.trusted.verify(&future, &signature).unwrap_err().code,
        ModelErrorCode::SchemaUnsupported
    );
}

/// A key that is not a key is a bug in this file, and it says so with the code
/// that means *this build cannot verify anything* rather than blaming a
/// catalog.
#[test]
fn a_malformed_pinned_key_is_a_trust_root_problem() {
    let error = TrustedKeys::from_public_keys(&["not a minisign key"]).unwrap_err();
    assert_eq!(error.code, ModelErrorCode::NoTrustRoot);
}

/// The shipped set is empty until Plan 1.2 mints the key, and the tree should
/// say so out loud rather than in a comment somebody deletes. When the key
/// lands, this assertion is what has to be updated — deliberately, so that
/// adding a trust root is a visible change.
#[test]
fn the_pinned_set_is_still_empty() {
    assert!(MODEL_SIGNING_KEYS.is_empty());
    assert!(TrustedKeys::pinned().is_empty());
}

// ----------------------------------------------------------- trust policy

/// **`Signed` is still the default**, and a store nobody said anything to gets
/// it. The MVP's second policy is additional, never a replacement.
#[test]
fn signed_is_the_default_policy() {
    assert_eq!(TrustPolicy::default(), TrustPolicy::SIGNED);
    assert!(!TrustPolicy::SIGNED.is_digest_only());
}

/// **A stable build cannot reach the digest-only arm**, whatever any caller
/// says — the guard is a compile-time constant of this crate, and this asserts
/// the *rule* it applies rather than the constant, so both answers are testable
/// in one build.
#[test]
fn a_stable_build_is_refused_the_digest_only_policy() {
    let refused = TrustPolicy::digest_only_on(Some("stable")).unwrap_err();
    assert_eq!(refused.code, ModelErrorCode::NoTrustRoot);
    assert!(
        refused.message.contains("model-signing key"),
        "{}",
        refused.message
    );
}

/// Every other build may — a beta, a nightly, an internal build, and a local
/// `cargo build` with no channel stamped into it at all. That is the MVP's
/// bargain, and the unconditional `warn!` in `install` is what makes it visible
/// rather than silent.
#[test]
fn every_other_channel_may_install_without_a_signature() {
    for channel in [
        Some("beta"),
        Some("nightly"),
        Some("internal-debug"),
        Some("internal-release"),
        None,
    ] {
        let policy = TrustPolicy::digest_only_on(channel)
            .unwrap_or_else(|e| panic!("{channel:?} was refused: {e}"));
        assert!(policy.is_digest_only(), "{channel:?}");
    }
}

/// The guard is the *compiled* channel and not a caller's word for it, so
/// `digest_only()` answers whatever this build is — which on a stable release
/// is a refusal nobody has to remember to write.
#[test]
fn the_guard_reads_the_channel_this_crate_was_compiled_for() {
    let expected = TrustPolicy::digest_only_on(option_env!("KORU_RELEASE_CHANNEL"));
    assert_eq!(
        TrustPolicy::digest_only().map(|p| p.is_digest_only()),
        expected.map(|p| p.is_digest_only())
    );
}
