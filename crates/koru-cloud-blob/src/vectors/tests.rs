// SPDX-License-Identifier: MPL-2.0

use super::*;

/// The vectors are read, not admired. If either file stops parsing into these
/// shapes the build says so, which is the point of compiling them in.
#[test]
fn both_vector_files_parse_and_hold_what_the_contract_says_they_hold() {
    let cases = cases();
    assert_eq!(
        cases.len(),
        50,
        "cases.json holds fifty admission decisions"
    );
    let sequences = sequences();
    assert_eq!(sequences.len(), 12, "sequences.json holds twelve orderings");

    let counts = |prefix: char| {
        cases
            .iter()
            .filter(|case| case.id.starts_with(prefix))
            .count()
    };
    assert_eq!((counts('C'), counts('R')), (16, 15));
    assert_eq!((counts('A'), counts('V')), (6, 3));
    assert_eq!((counts('G'), counts('P')), (4, 6));
}

/// Every id is unique and every expected code is one this crate's registry
/// knows, so a case can never quietly assert a code nothing produces.
#[test]
fn every_case_names_a_registered_code_and_a_unique_id() {
    let mut ids = std::collections::BTreeSet::new();
    for case in cases() {
        assert!(ids.insert(case.id.clone()), "duplicate case {}", case.id);
        if case.code != "ok" {
            assert!(
                crate::BlobCode::parse(&case.code).is_some(),
                "{} names {}",
                case.id,
                case.code
            );
        }
        assert!(!case.note.is_empty(), "{} has no note", case.id);
    }
    let mut sequence_ids = std::collections::BTreeSet::new();
    for sequence in sequences() {
        assert!(
            sequence_ids.insert(sequence.id.clone()),
            "duplicate sequence {}",
            sequence.id
        );
        assert!(!sequence.steps.is_empty(), "{} has no steps", sequence.id);
    }
}

/// The prefix of a case is which family it belongs to, and the `kind` field
/// says the same thing. Two ways to read one fact have to agree.
#[test]
fn the_id_prefix_and_the_kind_agree() {
    for case in cases() {
        let expected = match case.id.chars().next().unwrap() {
            'C' => CaseKind::Capability,
            'R' => CaseKind::Range,
            'A' => CaseKind::Allocate,
            'V' => CaseKind::Verify,
            'G' => CaseKind::Get,
            'P' => CaseKind::Report,
            other => panic!("case {} has prefix {other}", case.id),
        };
        assert_eq!(case.kind, expected, "{}", case.id);
    }
}

/// **`mapping_before_bytes: false` is the invariant that outranks the rest.**
/// Three sequences assert it from different angles and the loader must carry
/// it, or the sequence tests would be asserting nothing.
#[test]
fn the_ordering_invariants_survive_the_loader() {
    let sequences = sequences();
    let named = |id: &str| {
        sequences
            .iter()
            .find(|sequence| sequence.id == id)
            .unwrap_or_else(|| panic!("sequence {id}"))
            .clone()
    };
    for id in ["S01", "S04", "S09"] {
        assert_eq!(named(id).mapping_before_bytes, Some(false), "{id}");
    }
    for id in ["S10", "S11"] {
        assert_eq!(named(id).association_before_bytes, Some(false), "{id}");
    }
    for id in ["S02", "S05", "S10"] {
        assert_eq!(named(id).resends, Some(0), "{id}");
    }
    let racing = named("S08");
    assert_eq!((racing.sessions, racing.locators), (Some(1), Some(1)));
    // S07 and S09 must never reach durability, and `null` has to survive as a
    // decision rather than becoming "the field was absent".
    for id in ["S07", "S09"] {
        assert_eq!(named(id).durable_after, None, "{id}");
    }
    assert_eq!(named("S01").durable_after.as_deref(), Some("commit"));
    // S12's variant is the other half of one rule: a session that keeps
    // progressing is never reclaimed.
    let expiry = named("S12");
    assert_eq!(
        expiry.reclaims.as_deref(),
        Some(&["staging".to_string()][..])
    );
    assert_eq!(
        expiry.variant.expect("S12 carries its variant").reclaims,
        Some(Vec::new())
    );
}

/// Fixture identity is deterministic and admissible: a failing case names the
/// same id on every machine, and that id passes every check a real one would.
#[test]
fn a_schematic_label_becomes_one_stable_admissible_id() {
    assert_eq!(fixture_id("upload-1"), fixture_id("upload-1"));
    assert_ne!(fixture_id("upload-1"), fixture_id("upload-2"));
    assert_ne!(fixture_id("ws-1"), fixture_id("inc-1"));
    for label in ["upload-1", "upload-2", "loc-1", "ws-1", "ws-2", "inc-1"] {
        let id = fixture_id(label);
        assert!(id.is_v4(), "{label}");
        assert_eq!(
            id.to_string().len(),
            36,
            "{label} spells canonically hyphenated"
        );
    }
}

/// `b3:aa` is an address in a sentence; a real one is 64 hex characters. The
/// expansion is stable and is admissible wherever a real address would be.
#[test]
fn a_schematic_hash_becomes_one_stable_canonical_address() {
    let expanded = fixture_hash("b3:aa");
    assert_eq!(expanded, fixture_hash("b3:aa"));
    assert!(koru_account_session::cloud::blob::valid_blob_hash(
        &expanded
    ));
    assert_eq!(expanded.len(), 3 + 64);
    assert_ne!(fixture_hash("b3:aa"), fixture_hash("b3:bb"));
}
