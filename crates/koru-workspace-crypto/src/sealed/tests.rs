// SPDX-License-Identifier: MPL-2.0

//! `ContentCipherSuiteV1` against **Project Wycheproof**, plus the properties
//! Koru needs that a conformance suite does not assert.
//!
//! The vectors are the upstream ones for `c2sp.org/chunked-encryption`
//! Cobblestone-256, vendored under `vectors/wycheproof/` and embedded with
//! `include_str!` so the default suite touches no disk. They are the reason
//! this module can claim truncation, extension, reordering and malformed-length
//! behaviour rather than merely testing it against itself: **35 cases written
//! by somebody who did not write this implementation**, including the two
//! counter-rollover messages that catch a wrong nonce encoding.

use std::collections::BTreeSet;

use sha2::{Digest, Sha512};

use super::*;

const WYCHEPROOF: &str =
    include_str!("../../vectors/wycheproof/c2sp_chunked_encryption_aes_256_gcm_test.json");

fn unhex(s: &str) -> Vec<u8> {
    assert!(s.len().is_multiple_of(2), "hex must be even length");
    (0..s.len() / 2)
        .map(|i| u8::from_str_radix(&s[i * 2..i * 2 + 2], 16).expect("hex"))
        .collect()
}

fn inflate(compressed: &[u8]) -> Vec<u8> {
    use std::io::Read as _;
    let mut out = Vec::new();
    flate2::read::ZlibDecoder::new(compressed)
        .read_to_end(&mut out)
        .expect("wycheproof ciphertexts are zlib streams");
    out
}

struct Case {
    id: u64,
    comment: String,
    key: Vec<u8>,
    context: Vec<u8>,
    ciphertext: Vec<u8>,
    aead_key: Option<Vec<u8>>,
    base_nonce: Option<Vec<u8>>,
    msg_len: Option<u64>,
    msg_sha512: Option<Vec<u8>>,
    valid: bool,
    flags: BTreeSet<String>,
}

fn cases() -> Vec<Case> {
    let root: serde_json::Value = serde_json::from_str(WYCHEPROOF).expect("vector json");
    assert_eq!(root["algorithm"], "Cobblestone-256");
    let mut out = Vec::new();
    for group in root["testGroups"].as_array().expect("groups") {
        assert_eq!(group["aead"], "AEAD_AES_256_GCM");
        assert_eq!(group["sha"], "SHA-512");
        for test in group["tests"].as_array().expect("tests") {
            out.push(Case {
                id: test["tcId"].as_u64().expect("tcId"),
                comment: test["comment"].as_str().unwrap_or_default().to_string(),
                key: unhex(test["key"].as_str().expect("key")),
                context: unhex(test["ctx"].as_str().unwrap_or_default()),
                ciphertext: inflate(&unhex(test["ct"].as_str().expect("ct"))),
                aead_key: test["aeadKey"].as_str().map(unhex),
                base_nonce: test["baseNonce"].as_str().map(unhex),
                msg_len: test["msgLength"].as_u64(),
                msg_sha512: test["msgSha512"].as_str().map(unhex),
                valid: test["result"] == "valid",
                flags: test["flags"]
                    .as_array()
                    .map(|f| {
                        f.iter()
                            .filter_map(|v| v.as_str().map(str::to_string))
                            .collect()
                    })
                    .unwrap_or_default(),
            });
        }
    }
    assert!(out.len() >= 35, "the vendored suite lost cases");
    out
}

fn input_key(case: &Case) -> Option<InputKey> {
    // Cases 23 and 24 are `InvalidKeySize`: the input key is not 32 bytes.
    // `InputKey` is `[u8; 32]`, so the rejection is the type system's and the
    // conversion below is where it happens.
    let bytes: [u8; KEY_LEN] = case.key.clone().try_into().ok()?;
    Some(InputKey::from_bytes(bytes))
}

#[test]
fn wycheproof_valid_vectors_decrypt_to_the_promised_message() {
    for case in cases().iter().filter(|c| c.valid) {
        let key = input_key(case).unwrap_or_else(|| panic!("case {} key size", case.id));
        let plaintext = open(&key, &case.context, &case.ciphertext)
            .unwrap_or_else(|e| panic!("case {} ({}) failed: {e}", case.id, case.comment));
        assert_eq!(
            plaintext.len() as u64,
            case.msg_len.expect("valid vectors carry msgLength"),
            "case {} length",
            case.id
        );
        let digest: [u8; 64] = Sha512::digest(&plaintext).into();
        assert_eq!(
            &digest[..],
            &case
                .msg_sha512
                .clone()
                .expect("valid vectors carry msgSha512")[..],
            "case {} ({}) digest",
            case.id,
            case.comment
        );
    }
}

#[test]
fn wycheproof_invalid_vectors_are_all_refused() {
    for case in cases().iter().filter(|c| !c.valid) {
        let Some(key) = input_key(case) else {
            // The `InvalidKeySize` cases. There is no way to build an
            // `InputKey` from them, which is the rejection the vector asks for.
            assert!(
                case.flags.contains("InvalidKeySize"),
                "case {} produced no key and is not an InvalidKeySize case",
                case.id
            );
            continue;
        };
        let outcome = open(&key, &case.context, &case.ciphertext);
        assert!(
            outcome.is_err(),
            "case {} ({}) decrypted and must not have",
            case.id,
            case.comment
        );
    }
}

#[test]
fn wycheproof_pins_the_derived_key_and_base_nonce() {
    // The `aeadKey` and `baseNonce` columns are what pin the hand-written
    // HKDF-Expand. If it were wrong these would be wrong, and every decryption
    // above would fail for a reason nobody could localize.
    let mut checked = 0;
    for case in cases() {
        let (Some(expected_key), Some(expected_nonce)) = (&case.aead_key, &case.base_nonce) else {
            continue;
        };
        let Some(key) = input_key(&case) else {
            continue;
        };
        let mut salt = [0u8; SALT_LEN];
        if case.ciphertext.len() < SALT_LEN {
            continue;
        }
        salt.copy_from_slice(&case.ciphertext[..SALT_LEN]);
        let derived = derive(&key, &salt, &case.context);
        assert_eq!(&derived.key[..], &expected_key[..], "case {} key", case.id);
        assert_eq!(
            &derived.base_nonce[..],
            &expected_nonce[..],
            "case {} base nonce",
            case.id
        );
        checked += 1;
    }
    assert!(checked >= 25, "expected the suite to pin many derivations");
}

#[test]
fn the_counter_rollover_cases_are_present_and_pass() {
    let rollover: Vec<_> = cases()
        .into_iter()
        .filter(|c| c.flags.contains("CounterRollover"))
        .collect();
    assert_eq!(
        rollover.len(),
        2,
        "the two messages that cross a chunk-counter byte boundary must be exercised"
    );
    for case in rollover {
        let key = input_key(&case).expect("key");
        let plaintext = open(&key, &case.context, &case.ciphertext).expect("rollover");
        assert_eq!(plaintext.len() as u64, case.msg_len.unwrap());
    }
}

#[test]
fn a_valid_vector_re_encrypts_to_the_same_bytes() {
    // Encryption is deterministic given the salt, so a decrypt/re-encrypt round
    // trip proves the sealing path and not only the opening one.
    for case in cases().iter().filter(|c| c.valid) {
        let key = input_key(case).expect("key");
        let plaintext = open(&key, &case.context, &case.ciphertext).expect("open");
        let mut salt = [0u8; SALT_LEN];
        salt.copy_from_slice(&case.ciphertext[..SALT_LEN]);
        let resealed = seal(&key, &salt, &case.context, &plaintext).expect("seal");
        assert_eq!(resealed, case.ciphertext, "case {} reseal", case.id);
    }
}

// ------------------------------------------------ properties Koru relies on

#[test]
fn the_plaintext_length_of_a_valid_object_is_arithmetic() {
    // D40, narrowly: given a valid object's ciphertext length, the exact
    // plaintext length follows without a key. Empty, single byte, both sides
    // of a chunk boundary, multi-chunk, and the 10 GiB blob ceiling (D14) —
    // computed, not encrypted, because the point is that no bytes are needed.
    let ten_gib = 10u64 * 1024 * 1024 * 1024;
    for plaintext_len in [
        0,
        1,
        CHUNK_LEN as u64 - 1,
        CHUNK_LEN as u64,
        CHUNK_LEN as u64 + 1,
        2 * CHUNK_LEN as u64,
        2 * CHUNK_LEN as u64 + 1,
        1_000_000,
        ten_gib - 1,
        ten_gib,
        ten_gib + 1,
    ] {
        let forward = SealedLayout::of_plaintext_len(plaintext_len).expect("layout");
        let back = SealedLayout::of_ciphertext_len(forward.ciphertext_len()).expect("reverse");
        assert_eq!(back.plaintext_len(), plaintext_len);
        assert_eq!(back.chunk_count(), forward.chunk_count());
    }
}

#[test]
fn the_derived_length_matches_what_was_actually_sealed() {
    let key = InputKey::from_bytes([3u8; KEY_LEN]);
    for plaintext_len in [
        0usize,
        1,
        CHUNK_LEN - 1,
        CHUNK_LEN,
        CHUNK_LEN + 1,
        3 * CHUNK_LEN,
    ] {
        let plaintext = vec![0xa5u8; plaintext_len];
        let sealed = seal(&key, &[9u8; SALT_LEN], b"ctx", &plaintext).expect("seal");
        let layout = SealedLayout::of_ciphertext_len(sealed.len() as u64).expect("layout");
        assert_eq!(layout.plaintext_len(), plaintext_len as u64);
        assert_eq!(open(&key, b"ctx", &sealed).expect("open"), plaintext);
    }
}

#[test]
fn a_ciphertext_length_that_is_a_whole_number_of_full_chunks_is_truncated() {
    // The specification's own edge: `56 + k * (16 KiB + tag)` means the final
    // chunk was full length, which cannot happen for a well-formed message.
    for k in 1u64..4 {
        let len = SEALED_HEADER_LEN as u64 + k * CHUNK_CIPHERTEXT_LEN as u64;
        assert_eq!(
            SealedLayout::of_ciphertext_len(len).unwrap_err(),
            CryptoError::Malformed
        );
    }
}

#[test]
fn a_final_chunk_shorter_than_a_tag_is_refused() {
    for extra in 0..TAG_LEN as u64 {
        let len = SEALED_HEADER_LEN as u64 + CHUNK_CIPHERTEXT_LEN as u64 + extra;
        assert_eq!(
            SealedLayout::of_ciphertext_len(len).unwrap_err(),
            CryptoError::Malformed
        );
    }
}

#[test]
fn lengths_that_cannot_be_a_ciphertext_do_not_overflow_the_parser() {
    // D91: malformed lengths must not overflow. Every one of these is a value
    // an attacker controls in a length field somewhere upstream.
    for len in [0u64, 1, 55, 56, u64::MAX, u64::MAX - 1, i64::MAX as u64] {
        let outcome = SealedLayout::of_ciphertext_len(len);
        assert!(outcome.is_err(), "{len} must not parse");
    }
    // And one that is arithmetically consistent but past the chunk ceiling.
    let past_the_ceiling =
        SEALED_HEADER_LEN as u64 + (MAX_CHUNKS + 1) * CHUNK_CIPHERTEXT_LEN as u64 - 1;
    assert_eq!(
        SealedLayout::of_ciphertext_len(past_the_ceiling).unwrap_err(),
        CryptoError::TooLarge
    );
}

#[test]
fn a_wrong_context_fails_at_the_commitment_before_any_chunk() {
    let key = InputKey::from_bytes([1u8; KEY_LEN]);
    let sealed = seal(&key, &[2u8; SALT_LEN], b"workspace-a", b"secret").expect("seal");
    assert_eq!(
        open(&key, b"workspace-b", &sealed).unwrap_err(),
        CryptoError::Commitment
    );
    // A one-byte change in the context is enough.
    assert_eq!(
        open(&key, b"workspace-A", &sealed).unwrap_err(),
        CryptoError::Commitment
    );
}

#[test]
fn a_wrong_key_fails_at_the_commitment() {
    let key = InputKey::from_bytes([1u8; KEY_LEN]);
    let sealed = seal(&key, &[2u8; SALT_LEN], b"ctx", b"secret").expect("seal");
    let mut other = [1u8; KEY_LEN];
    other[31] ^= 1;
    assert_eq!(
        open(&InputKey::from_bytes(other), b"ctx", &sealed).unwrap_err(),
        CryptoError::Commitment
    );
}

#[test]
fn random_access_reaches_any_chunk_and_refuses_a_transplanted_one() {
    let key = InputKey::from_bytes([4u8; KEY_LEN]);
    let plaintext: Vec<u8> = (0..3 * CHUNK_LEN + 7).map(|i| (i % 251) as u8).collect();
    let sealed = seal(&key, &[5u8; SALT_LEN], b"ctx", &plaintext).expect("seal");
    let opener = Opener::new(&key, b"ctx", sealed.len() as u64, &sealed).expect("opener");
    assert_eq!(opener.layout().chunk_count(), 4);

    // Every chunk opens at the offset the layout predicts, in any order.
    for index in [3u64, 0, 2, 1] {
        let (offset, len) = opener.layout().chunk_at(index).expect("chunk");
        let slice = &sealed[offset as usize..(offset + len) as usize];
        let chunk = opener.open_chunk(index, slice).expect("open chunk");
        let start = index as usize * CHUNK_LEN;
        let end = usize::min(start + CHUNK_LEN, plaintext.len());
        assert_eq!(chunk, &plaintext[start..end]);
    }

    // Chunk 1's bytes presented as chunk 0 decrypt under the wrong nonce.
    let (offset, len) = opener.layout().chunk_at(1).expect("chunk");
    let slice = &sealed[offset as usize..(offset + len) as usize];
    assert_eq!(opener.open_chunk(0, slice).unwrap_err(), CryptoError::Aead);
}

#[test]
fn an_empty_message_is_one_empty_chunk() {
    let key = InputKey::from_bytes([6u8; KEY_LEN]);
    let sealed = seal(&key, &[7u8; SALT_LEN], b"", b"").expect("seal");
    assert_eq!(sealed.len(), SEALED_HEADER_LEN + TAG_LEN);
    assert_eq!(open(&key, b"", &sealed).expect("open"), Vec::<u8>::new());
}

#[test]
fn the_nonce_is_the_base_nonce_xor_a_big_endian_counter() {
    let base = [0u8; NONCE_LEN];
    assert_eq!(chunk_nonce(&base, 0), [0u8; NONCE_LEN]);
    assert_eq!(chunk_nonce(&base, 1), [0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 1]);
    assert_eq!(
        chunk_nonce(&base, 256),
        [0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 1, 0]
    );
    // XOR, not addition: applying the same counter twice is the identity.
    let base = [0xffu8; NONCE_LEN];
    let once = chunk_nonce(&base, 0x0123_4567);
    assert_ne!(once, base);
    let mut twice = once;
    for (n, c) in twice[NONCE_LEN - 8..]
        .iter_mut()
        .zip(0x0123_4567u64.to_be_bytes().iter())
    {
        *n ^= *c;
    }
    assert_eq!(twice, base);
}
