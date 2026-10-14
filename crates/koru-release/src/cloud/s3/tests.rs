// SPDX-License-Identifier: MPL-2.0

use super::*;

/// **RFC 4231, test case 1.** The point of pinning against a published vector
/// rather than against a value this file produced: every signature in this
/// module is four HMACs deep, so a typo in the primitive would make each of
/// them wrong *consistently*, and a self-referential assertion would pass.
#[test]
fn hmac_sha256_matches_rfc_4231_case_1() {
    let key = [0x0bu8; 20];
    assert_eq!(
        hex(&hmac(&key, b"Hi There")),
        "b0344c61d8db38535ca8afceaf0bf12b881dc200c9833da726e9376c2e32cff7"
    );
}

/// RFC 4231, test case 2 — a short ASCII key, which is the shape the SigV4
/// chain actually starts from.
#[test]
fn hmac_sha256_matches_rfc_4231_case_2() {
    assert_eq!(
        hex(&hmac(b"Jefe", b"what do ya want for nothing?")),
        "5bdcc146bf60754e6a042426089575c75a003f089d2739839dec58b964ec3843"
    );
}

/// The empty-body hash, which every `GET`, `DELETE` and `CreateMultipartUpload`
/// in this module signs.
#[test]
fn the_empty_payload_hash_is_the_published_one() {
    assert_eq!(
        sha256_hex(b""),
        "e3b0c44298fc1c149afbf4c8996fb92427ae41e4649b934ca495991b7852b855"
    );
}

/// The documented R2-token fallback, stated as two separable claims so a wrong
/// one names itself. The secret is the SHA-256 of the **token value**, not of
/// the id, and the id travels verbatim.
#[test]
fn s3_credentials_are_derived_from_an_r2_api_token() {
    let credentials = S3Credentials::derive("abc123", "a-token-value");
    assert_eq!(credentials.access_key_id, "abc123");
    assert_eq!(credentials.secret_access_key, sha256_hex(b"a-token-value"));
}

#[test]
fn explicit_s3_credentials_are_used_verbatim() {
    let credentials = S3Credentials::from_explicit_pair(
        Some("access-id".to_string()),
        Some("secret-value".to_string()),
    )
    .unwrap()
    .unwrap();
    assert_eq!(credentials.access_key_id, "access-id");
    assert_eq!(credentials.secret_access_key, "secret-value");
}

#[test]
fn an_explicit_s3_credential_cannot_be_half_configured() {
    let error = S3Credentials::from_explicit_pair(Some("access-id".to_string()), None).unwrap_err();
    assert!(error.contains("set both"));
    assert!(
        S3Credentials::from_explicit_pair(None, None)
            .unwrap()
            .is_none()
    );
}

/// A `{:?}` in a hurry must not print the secret.
#[test]
fn the_secret_access_key_is_not_in_the_debug_output() {
    let credentials = S3Credentials::derive("abc123", "a-token-value");
    let rendered = format!("{credentials:?}");
    assert!(rendered.contains("abc123"));
    assert!(!rendered.contains(&sha256_hex(b"a-token-value")));
    assert!(rendered.contains("redacted"));
}

/// **The bytes that are signed, as literal text.**
///
/// Asserted verbatim rather than through its hash so a reviewer can read what
/// goes into the signature: the trailing blank line after the headers, the
/// semicolon-separated signed-header list, the sorted query, and the encoded
/// path with its slashes intact are each a separate way to get this wrong and
/// each is visible here.
#[test]
fn the_canonical_request_is_the_text_s3_specifies() {
    let headers = vec![
        (
            "host".to_string(),
            "acct.r2.cloudflarestorage.com".to_string(),
        ),
        ("x-amz-content-sha256".to_string(), sha256_hex(b"")),
        ("x-amz-date".to_string(), "20260830T120000Z".to_string()),
    ];
    let request = CanonicalRequest {
        method: "POST",
        path: "/koru-updates/models/parakeet-tdt-v3/1.0.0/dead.zip",
        query: &[("uploads", "")],
        headers: &headers,
        payload_sha256: &sha256_hex(b""),
    };
    let expected = concat!(
        "POST\n",
        "/koru-updates/models/parakeet-tdt-v3/1.0.0/dead.zip\n",
        "uploads=\n",
        "host:acct.r2.cloudflarestorage.com\n",
        "x-amz-content-sha256:e3b0c44298fc1c149afbf4c8996fb92427ae41e4649b934ca495991b7852b855\n",
        "x-amz-date:20260830T120000Z\n",
        "\n",
        "host;x-amz-content-sha256;x-amz-date\n",
        "e3b0c44298fc1c149afbf4c8996fb92427ae41e4649b934ca495991b7852b855"
    );
    assert_eq!(request.render(), expected);
    assert_eq!(
        request.signed_headers(),
        "host;x-amz-content-sha256;x-amz-date"
    );
}

/// Query parameters are sorted by their **encoded** name, and a valueless one
/// still carries its `=`. Both are rules a hand-written signer skips.
#[test]
fn the_query_string_is_sorted_and_every_key_carries_an_equals() {
    let headers = vec![("host".to_string(), "h".to_string())];
    let request = CanonicalRequest {
        method: "PUT",
        path: "/b/k",
        query: &[("uploadId", "xyz"), ("partNumber", "2")],
        headers: &headers,
        payload_sha256: "00",
    };
    let rendered = request.render();
    let query = rendered.lines().nth(2).unwrap();
    assert_eq!(query, "partNumber=2&uploadId=xyz");
}

/// The string-to-sign carries the scope, and the scope carries `auto` — a real
/// AWS region name here signs a credential R2 refuses.
#[test]
fn the_scope_is_the_regionless_one_r2_wants() {
    assert_eq!(scope("20260830"), "20260830/auto/s3/aws4_request");
    let to_sign = string_to_sign("20260830T120000Z", "20260830", "canonical");
    assert_eq!(
        to_sign,
        format!(
            "AWS4-HMAC-SHA256\n20260830T120000Z\n20260830/auto/s3/aws4_request\n{}",
            sha256_hex(b"canonical")
        )
    );
}

/// A signature is 64 hex characters, names the credential and the signed
/// headers, and **moves when the payload does**. The last clause is the one
/// worth asserting: a signer that dropped the payload hash would still produce
/// a well-formed header, and only the origin would ever say so.
#[test]
fn the_authorization_header_binds_the_payload() {
    let credentials = S3Credentials::derive("kid", "secret");
    let headers = vec![("host".to_string(), "h".to_string())];
    let mine = |payload: &str| {
        let request = CanonicalRequest {
            method: "PUT",
            path: "/b/k",
            query: &[],
            headers: &headers,
            payload_sha256: payload,
        };
        authorization(&credentials, "20260830T120000Z", "20260830", &request)
    };
    let one = mine(&sha256_hex(b"one"));
    let two = mine(&sha256_hex(b"two"));
    assert!(one.starts_with("AWS4-HMAC-SHA256 Credential=kid/20260830/auto/s3/aws4_request, "));
    assert!(one.contains("SignedHeaders=host, Signature="));
    assert_ne!(one, two);
    let signature = one.rsplit("Signature=").next().unwrap();
    assert_eq!(signature.len(), 64);
    assert!(signature.bytes().all(|b| b.is_ascii_hexdigit()));
    // Deterministic: the same inputs sign the same way, which is what makes a
    // retry a retry rather than a new request.
    assert_eq!(one, mine(&sha256_hex(b"one")));
}

/// S3's encoding is RFC 3986's, not a form encoder's. A space is `%20` and
/// never `+`, and `~` is left alone.
#[test]
fn uri_encoding_is_rfc_3986_and_not_a_form_encoder() {
    assert_eq!(uri_encode("a b~c", false), "a%20b~c");
    assert_eq!(uri_encode("a/b", false), "a%2Fb");
    assert_eq!(uri_encode("a/b", true), "a/b");
}

/// The part plan for a real artifact: 475,273,363 bytes at 64 MiB is eight
/// parts, the first seven identical and the last short. R2 requires exactly
/// that shape.
#[test]
fn a_nemotron_export_splits_into_eight_equal_parts_and_a_remainder() {
    let plan = parts(475_273_363, PART_SIZE);
    assert_eq!(plan.len(), 8);
    for (index, (number, offset, length)) in plan.iter().enumerate() {
        assert_eq!(*number as usize, index + 1);
        assert_eq!(*offset, index as u64 * PART_SIZE);
        if index < 7 {
            assert_eq!(*length, PART_SIZE);
        }
    }
    let last = plan.last().unwrap();
    assert!(last.2 < PART_SIZE && last.2 > 0);
    assert_eq!(plan.iter().map(|part| part.2).sum::<u64>(), 475_273_363);
}

/// The two edges: an exact multiple must not produce a trailing empty part, and
/// a file smaller than one part is one part rather than none.
#[test]
fn the_part_plan_has_no_empty_part_and_no_missing_one() {
    assert_eq!(parts(PART_SIZE * 3, PART_SIZE).len(), 3);
    assert_eq!(parts(1, PART_SIZE), vec![(1, 0, 1)]);
    assert!(parts(0, PART_SIZE).is_empty());
}

/// The threshold is below the documented cap, and the cap is what the REST
/// endpoint states. A change to either is a change somebody has to mean.
#[test]
fn the_multipart_threshold_stays_under_the_rest_limit() {
    assert!(MULTIPART_ABOVE_BYTES < REST_UPLOAD_MAX_BYTES);
    assert_eq!(REST_UPLOAD_MAX_BYTES, 300 * 1000 * 1000);
    // Every artifact this publisher carries is over it, which is the finding
    // that made this module necessary rather than optional.
    assert!(475_273_363 > MULTIPART_ABOVE_BYTES);
}

/// One tag, out of a reply we already authenticated.
#[test]
fn the_upload_id_is_read_out_of_the_xml() {
    let body = "<?xml version=\"1.0\"?><InitiateMultipartUploadResult>\
                <Bucket>koru-updates</Bucket><Key>models/x</Key>\
                <UploadId>ABC-123</UploadId></InitiateMultipartUploadResult>";
    assert_eq!(xml_tag(body, "UploadId").as_deref(), Some("ABC-123"));
    assert_eq!(xml_tag(body, "NoSuchTag"), None);
}
