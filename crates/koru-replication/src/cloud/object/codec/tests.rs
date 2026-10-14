// SPDX-License-Identifier: MPL-2.0

use super::*;
use crate::cloud::object::tests::*;

#[test]
fn individually_valid_vectors_fit_the_aggregate_header() {
    let mut version = VersionVector::default();
    for peer in 1_u64..1024 {
        version.set_last(loro::ID::new(peer.wrapping_mul(0x9e3779b97f4a7c15), 1));
        if version.encode().len() >= 3000 {
            break;
        }
    }
    let version = version.encode();
    assert!((3000..=4096).contains(&version.len()));
    let golden = goldens()
        .into_iter()
        .find(|golden| {
            let raw = bytes(golden["envelope"].as_str().unwrap());
            decode(&raw, limits()).unwrap().metadata().kind == Kind::Segment
        })
        .unwrap();
    let raw = bytes(golden["envelope"].as_str().unwrap());
    let decoded = decode(&raw, limits()).unwrap();
    let body = Body {
        predecessor_version: &version,
        result_version: &version,
        ..decoded.body()
    };
    let old = Limits::new(4096, 4096, 16_777_216, 16_777_216).unwrap();
    assert_eq!(encode(decoded.metadata(), body, old), Err(Error::TooLarge));
    let consistent = Limits::for_vector_capacity(4096, 16_777_216, 16_777_216).unwrap();
    let encoded = encode(decoded.metadata(), body, consistent).unwrap();
    let restored = decode(&encoded, consistent).unwrap();
    assert_eq!(restored.body().result_version, version);
    assert_eq!(consistent.header, 8465);
    let small_vectors = Limits::for_vector_capacity(128, 16_777_216, 16_777_216).unwrap();
    assert_eq!(
        encode(decoded.metadata(), body, small_vectors),
        Err(Error::TooLarge)
    );
    assert_eq!(
        Limits::for_vector_capacity(u32::MAX, 1, 1).unwrap_err(),
        Error::InvalidLimits
    );
}

#[test]
fn f05_kind_specific_optionals_coverage_padding_and_zero_payload_layers() {
    for golden in goldens().into_iter().take(10) {
        let raw = bytes(golden["envelope"].as_str().unwrap());
        let decoded = decode(&raw, limits()).unwrap();
        let m = decoded.metadata();
        let body = decoded.body();
        let mut bad = body;
        bad.result_version = &[];
        assert_eq!(encode(m, bad, limits()), Err(Error::CausalShape));
        let mut bad = body;
        if m.kind == Kind::Segment {
            bad.predecessor_version = &[];
        } else {
            bad.checkpoint_frontiers = &[];
        }
        assert_eq!(encode(m, bad, limits()), Err(Error::CausalShape));
        let mut bad = body;
        if m.kind == Kind::Segment {
            bad.checkpoint_frontiers = &[0];
        } else {
            bad.predecessor_version = &[0];
        }
        assert_eq!(encode(m, bad, limits()), Err(Error::CausalShape));
        let mut bad_meta = m;
        bad_meta.covers_through_sequence = if m.kind == Kind::Segment {
            1
        } else {
            m.sequence
        };
        assert_eq!(encode(bad_meta, body, limits()), Err(Error::CausalShape));
        let header = u32::from_be_bytes(raw[10..14].try_into().unwrap()) as usize;
        let mut padded = raw.clone();
        padded.insert(22 + header, 0);
        padded[10..14].copy_from_slice(&((header + 1) as u32).to_be_bytes());
        assert_eq!(decode(&padded, limits()).unwrap_err(), Error::Length);
        let mut empty_payload = body;
        empty_payload.payload = &[];
        let empty = encode(m, empty_payload, limits()).unwrap();
        assert_eq!(
            decode(&empty, limits())
                .unwrap()
                .commitments()
                .payload_byte_length,
            0
        );
        // Byte-layer length 0 is representable. Real Loro import/SQL receipt
        // acceptance is a different boundary; this is not an empty checkpoint.
        let doc = loro::LoroDoc::new();
        assert!(doc.import(&[]).is_err());
    }
}

#[test]
fn frozen_families_one_two_four_exact_bytes_and_independent_commitments() {
    for golden in goldens() {
        let raw = bytes(golden["envelope"].as_str().unwrap());
        let expected = expected(&golden["expected"]);
        let decoded = decode(&raw, limits()).unwrap();
        assert_eq!(decoded.metadata(), expected.metadata, "{}", golden["id"]);
        assert_eq!(
            decoded.commitments(),
            expected.commitments,
            "{}",
            golden["id"]
        );
        assert_eq!(
            logical_key(decoded.metadata()).unwrap(),
            expected.logical_key
        );
        decoded.bind(expected).unwrap();
        assert_eq!(
            encode(decoded.metadata(), decoded.body(), limits()).unwrap(),
            raw
        );
        assert_eq!(
            decoded.body().payload,
            bytes(golden["expected"]["payload"].as_str().unwrap())
        );
    }
}

#[test]
fn malformed_prefixes_and_exact_consumption_refuse_before_loro() {
    let raw = bytes(goldens()[0]["envelope"].as_str().unwrap());
    for n in 0..22 {
        assert!(matches!(decode(&raw[..n], limits()), Err(Error::Truncated)));
    }
    for n in 22..raw.len() {
        assert!(matches!(decode(&raw[..n], limits()), Err(Error::Length)));
    }
    for (offset, value, expected) in [
        (0, 0, Error::Magic),
        (9, 2, Error::Version),
        (22, 3, Error::Tag),
        (23, 3, Error::Tag),
        (24, 2, Error::Region),
        (25, 6, Error::Unit),
        (114, 2, Error::Unit),
    ] {
        let mut bad = raw.clone();
        bad[offset] = value;
        assert!(
            matches!(decode(&bad, limits()), Err(e) if e == expected),
            "offset {offset}"
        );
    }
    let mut extra = raw.clone();
    extra.push(0);
    assert!(matches!(decode(&extra, limits()), Err(Error::Length)));
    let mut payload = raw.clone();
    *payload.last_mut().unwrap() ^= 1;
    assert!(matches!(decode(&payload, limits()), Err(Error::Checksum)));
}

#[test]
fn malformed_encoded_vectors_reach_loro_with_valid_lengths_and_payload_checksum() {
    for golden in goldens().into_iter().take(2) {
        let raw = bytes(golden["envelope"].as_str().unwrap());
        let mut cursor = 227; // first singleton vector length, data starts 231
        for _ in 0..3 {
            let size = u32::from_be_bytes(raw[cursor..cursor + 4].try_into().unwrap()) as usize;
            cursor += 4;
            if size > 0 {
                let mut malformed = raw.clone();
                malformed[cursor..cursor + size].fill(255); // unterminated/overflow varint
                assert_eq!(decode(&malformed, limits()).unwrap_err(), Error::Loro);
                assert_eq!(&malformed[10..22], &raw[10..22]); // declarations unchanged
                assert_eq!(
                    &malformed[22 + golden["expected"]["header"].as_str().unwrap().len() / 2..],
                    &raw[22 + golden["expected"]["header"].as_str().unwrap().len() / 2..]
                );
            }
            cursor += size;
        }
    }
}

#[test]
fn all_integer_domains_and_header_vector_bounds_are_checked_without_large_allocations() {
    assert_eq!(
        preflight_length(HEADER_MAX, 0, limits()),
        Ok(22 + u64::from(HEADER_MAX))
    );
    assert_eq!(
        preflight_length(HEADER_MAX + 1, 0, limits()),
        Err(Error::TooLarge)
    );
    assert_eq!(
        preflight_length(0, i64::MAX as u64, limits()),
        Err(Error::Integer)
    );
    assert_eq!(
        preflight_length(0, i64::MAX as u64 + 1, limits()),
        Err(Error::Integer)
    );
    let raw = bytes(goldens()[0]["envelope"].as_str().unwrap());
    let decoded = decode(&raw, limits()).unwrap();
    for (size, refusal) in [(KEY_MAX, Error::Key), (KEY_MAX + 1, Error::TooLarge)] {
        let key = "x".repeat(size); // tiny declaration/grammar test, not valid UUID key
        assert_eq!(
            decoded
                .bind(ExpectedObject {
                    metadata: decoded.metadata(),
                    logical_key: &key,
                    commitments: decoded.commitments()
                })
                .unwrap_err(),
            refusal
        );
    }
    for offset in [106, 147, 171, 179, 187] {
        for number in [0_u64, i64::MAX as u64 + 1] {
            let mut bad = raw.clone();
            bad[offset..offset + 8].copy_from_slice(&number.to_be_bytes());
            assert!(
                matches!(decode(&bad, limits()), Err(Error::Integer)),
                "offset {offset}"
            );
        }
    }
    let mut huge_vector = raw.clone();
    huge_vector[227..231].copy_from_slice(&(VECTOR_MAX + 1).to_be_bytes());
    assert!(matches!(
        decode(&huge_vector, limits()),
        Err(Error::TooLarge)
    ));
    assert!(matches!(
        decode(&raw, Limits::new(1, 1, 1, 1).unwrap()),
        Err(Error::TooLarge)
    ));
    assert!(matches!(
        decode(&raw, Limits::new(HEADER_MAX, VECTOR_MAX, 1, 1).unwrap()),
        Err(Error::TooLarge)
    ));
    assert!(matches!(
        encode(
            decoded.metadata(),
            Body {
                result_version: &[],
                ..decoded.body()
            },
            limits()
        ),
        Err(Error::CausalShape)
    ));
}

#[test]
fn every_expected_metadata_and_commitment_axis_is_independently_bound() {
    let golden = goldens().remove(0);
    let raw = bytes(golden["envelope"].as_str().unwrap());
    let decoded = decode(&raw, limits()).unwrap();
    let expected = expected(&golden["expected"]);
    let foreign = CloudId::from_bytes([42; 16]);
    let m = expected.metadata;
    let mut axes = vec![
        Metadata {
            kind: Kind::Checkpoint,
            ..m
        },
        Metadata {
            unit: Unit::ProfilePrivate,
            ..m
        },
        Metadata {
            cloud_unit_id: foreign,
            ..m
        },
        Metadata {
            object_id: foreign,
            ..m
        },
        Metadata {
            sequence: m.sequence + 1,
            ..m
        },
        Metadata {
            writer_worker_id: foreign,
            ..m
        },
        Metadata {
            writer_lease_id: foreign,
            ..m
        },
        Metadata {
            writer_lease_epoch: m.writer_lease_epoch + 1,
            ..m
        },
        Metadata {
            authority_workspace_id: foreign,
            ..m
        },
        Metadata {
            authorization_version: m.authorization_version + 1,
            ..m
        },
        Metadata {
            membership_epoch: m.membership_epoch + 1,
            ..m
        },
        Metadata {
            cloud_access_epoch: m.cloud_access_epoch + 1,
            ..m
        },
        Metadata {
            covers_through_sequence: 1,
            ..m
        },
    ];
    if let CloudReplicaPrincipalV1::Workspace {
        workspace_id,
        incarnation_id,
        logical_replica_id,
    } = m.dataset
    {
        for dataset in [
            CloudReplicaPrincipalV1::Workspace {
                workspace_id: foreign,
                incarnation_id,
                logical_replica_id,
            },
            CloudReplicaPrincipalV1::Workspace {
                workspace_id,
                incarnation_id: foreign,
                logical_replica_id,
            },
            CloudReplicaPrincipalV1::Workspace {
                workspace_id,
                incarnation_id,
                logical_replica_id: foreign,
            },
        ] {
            axes.push(Metadata { dataset, ..m });
        }
    }
    for metadata in axes {
        assert!(matches!(
            decoded.bind(ExpectedObject {
                metadata,
                ..expected
            }),
            Err(Error::Binding)
        ));
    }
    for field in 0..9 {
        let mut c = expected.commitments;
        match field {
            0 => c.unit_ref_digest[0] ^= 1,
            1 => c.predecessor_version_digest = None,
            2 => c.result_version_digest[0] ^= 1,
            3 => c.checkpoint_frontier_digest = Some([0; 32]),
            4 => c.payload_sha256[0] ^= 1,
            5 => c.object_sha256[0] ^= 1,
            6 => c.request_digest[0] ^= 1,
            7 => c.payload_byte_length += 1,
            _ => c.object_byte_length += 1,
        }
        assert!(matches!(
            decoded.bind(ExpectedObject {
                commitments: c,
                ..expected
            }),
            Err(Error::Binding)
        ));
    }
    for key in [
        "",
        "../object",
        "w//object",
        expected.logical_key.trim_end_matches(".loro"),
    ] {
        assert!(matches!(
            decoded.bind(ExpectedObject {
                logical_key: key,
                ..expected
            }),
            Err(Error::Key)
        ));
    }
}
