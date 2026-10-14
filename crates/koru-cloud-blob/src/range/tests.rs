// SPDX-License-Identifier: MPL-2.0

use super::*;
use crate::vectors;

/// `cases.json`'s fifteen `R` rows, driven as the pure decision they are. The
/// HTTP adapter drives the same rows again against a live worker; this one
/// fails first and says which clause moved.
#[test]
fn every_range_vector_decides_as_the_contract_says() {
    let mut seen = 0;
    for case in vectors::cases() {
        if case.kind != vectors::CaseKind::Range {
            continue;
        }
        seen += 1;
        let capability_length = case.cap_len.or(case.total).unwrap_or(0);
        let outcome = match (&case.raw, case.range) {
            (Some(raw), _) => parse_content_range(raw).map(|_| unreachable!()),
            (None, Some([start, end])) => {
                let total = case.total.expect("a range case declares its total");
                let header = format!("bytes {start}-{end}/{total}");
                parse_content_range(&header)
                    .and_then(|range| {
                        let declared = (end - start + 1).max(0) as u64;
                        admit_range(
                            range,
                            case.committed.expect("a range case declares its offset"),
                            capability_length,
                            case.body_len.unwrap_or(declared),
                        )
                    })
                    // The decision cannot see the bytes, so the comparison a
                    // replay owes is the caller's — and R05 is the case that
                    // says a caller which skips it has admitted a conflict.
                    .and_then(|outcome| match outcome {
                        RangeOutcome::Replay { .. } if case.identical == Some(false) => {
                            Err(BlobCode::RangeConflict)
                        }
                        outcome => Ok(outcome),
                    })
            }
            (None, None) => panic!("range case {} carries neither range nor raw", case.id),
        };
        match (case.expected(), outcome) {
            (Some(expected), Err(actual)) => {
                assert_eq!(actual, expected, "{} — {}", case.id, case.note)
            }
            (None, Ok(outcome)) => {
                // R04 is the only admitted case whose offset does not move, and
                // it only moves nothing because the bytes are already there.
                assert_eq!(
                    outcome.committed_after(),
                    case.committed_after
                        .expect("an admitted range case declares the offset after"),
                    "{} — {}",
                    case.id,
                    case.note
                );
                if case.identical == Some(true) {
                    assert!(
                        matches!(outcome, RangeOutcome::Replay { .. }),
                        "{} is a replay",
                        case.id
                    );
                }
            }
            (expected, actual) => panic!("{} — {}: {expected:?} vs {actual:?}", case.id, case.note),
        }
    }
    assert_eq!(seen, 15, "every R row of cases.json was exercised");
}

/// A replay whose bytes differ is the caller's to refuse, and the outcome says
/// which comparison it owes — the decision alone cannot know.
#[test]
fn a_replay_names_the_bytes_the_caller_must_compare() {
    let range = parse_content_range("bytes 0-2097151/4194304").unwrap();
    let outcome = admit_range(range, 2_097_152, 4_194_304, 2_097_152).unwrap();
    assert_eq!(
        outcome,
        RangeOutcome::Replay {
            start: 0,
            length: 2_097_152,
            committed_after: 2_097_152
        }
    );
}

/// One spelling per number. A leading zero and a `+` are second spellings, and
/// two spellings of one offset is two answers to "where did we get to".
#[test]
fn only_the_canonical_spelling_of_an_offset_parses() {
    assert!(parse_content_range("bytes 0-10/100").is_ok());
    for bad in [
        "bytes 00-10/100",
        "bytes +0-10/100",
        "bytes 0-010/100",
        "bytes 0-10/0100",
        "bytes 0-10/ 100",
        "bytes  0-10/100",
        "bytes 0-10",
        "bytes */100",
        "bytes 0-10/100 ",
        "BYTES 0-10/100",
        "bytes abc/def",
        "",
    ] {
        assert_eq!(
            parse_content_range(bad).unwrap_err(),
            BlobCode::MalformedRange,
            "{bad:?}"
        );
    }
}

/// The final chunk is short, not oversized, and is admitted at exactly the last
/// byte of the declared total.
#[test]
fn the_final_chunk_is_admitted_at_the_last_byte_and_never_past_it() {
    let total = CHUNK + 100;
    let last = parse_content_range(&format!("bytes {CHUNK}-{}/{total}", total - 1)).unwrap();
    assert_eq!(
        admit_range(last, CHUNK, total, 100)
            .unwrap()
            .committed_after(),
        total
    );
    let past = parse_content_range(&format!("bytes {CHUNK}-{total}/{total}")).unwrap();
    assert_eq!(
        admit_range(past, CHUNK, total, 101).unwrap_err(),
        BlobCode::MalformedRange
    );
}

/// A chunk of exactly `CHUNK` is admitted and one byte more is not, so the
/// derived bound is the bound and not an approximation of it.
#[test]
fn the_chunk_ceiling_is_exact() {
    let total = 4 * CHUNK;
    let exact = parse_content_range(&format!("bytes 0-{}/{total}", CHUNK - 1)).unwrap();
    assert!(admit_range(exact, 0, total, CHUNK).is_ok());
    let over = parse_content_range(&format!("bytes 0-{}/{total}", CHUNK)).unwrap();
    assert_eq!(
        admit_range(over, 0, total, CHUNK + 1).unwrap_err(),
        BlobCode::MalformedRange
    );
}
