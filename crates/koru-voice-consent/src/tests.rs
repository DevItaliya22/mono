// SPDX-License-Identifier: MPL-2.0

use super::*;

fn terms() -> Terms {
    Terms {
        model: "parakeet-tdt-v3".into(),
        version: "1.2.0".into(),
        licence: "Apache-2.0".into(),
        attribution: "k2-fsa, Apache-2.0".into(),
        download_bytes: 648_291_840,
    }
}

fn scratch() -> (tempfile::TempDir, PathBuf) {
    let dir = tempfile::tempdir().expect("a temporary directory");
    let path = path_in(dir.path());
    (dir, path)
}

#[test]
fn nothing_written_is_never_asked() {
    let (_dir, path) = scratch();
    assert_eq!(read(&path), Consent::Absent(Reason::NeverAsked));
}

/// **The plan's test.** A restart is a second `read` of the same path by a
/// process that shares nothing else with the first.
#[test]
fn consent_survives_a_restart() {
    let (_dir, path) = scratch();
    grant(&path, terms(), 1_700_000_000).expect("the grant is written");

    let reopened = read(&path);
    let granted = reopened.granted().expect("consent is still here");
    assert_eq!(granted.terms(), &terms());
    assert_eq!(granted.granted_at(), 1_700_000_000);
}

#[test]
fn two_licences_are_recorded_and_cover_their_own_models() {
    let (_dir, path) = scratch();
    let whisper = Terms {
        model: "whisper-base".into(),
        licence: "MIT".into(),
        download_bytes: 57 * 1024 * 1024,
        ..terms()
    };
    let nemotron = Terms {
        model: "nemotron-160ms".into(),
        licence: "OpenMDW-1.1".into(),
        download_bytes: 475 * 1024 * 1024,
        ..terms()
    };
    grant_all(&path, vec![whisper.clone(), nemotron.clone()], 7).unwrap();
    let consent = read(&path);
    let granted = consent.granted().unwrap();
    assert!(granted.covers(&whisper));
    assert!(granted.covers(&nemotron));
    assert_eq!(granted.all_terms().count(), 2);
}

#[test]
fn a_v1_single_grant_migrates_in_memory() {
    let (_dir, path) = scratch();
    let old = serde_json::json!({
        "format": FORMAT,
        "schema_version": 1,
        "granted": {
            "terms": terms(),
            "granted_at": 42
        }
    });
    std::fs::write(&path, serde_json::to_vec(&old).unwrap()).unwrap();
    let consent = read(&path);
    let granted = consent.granted().expect("the old grant remains valid");
    assert!(granted.covers(&terms()));
    assert_eq!(granted.granted_at(), 42);
}

/// **A torn write is not read as consent.** The write itself is atomic, so this
/// arranges what a torn one would leave: a prefix of the bytes under the
/// record's own name.
#[test]
fn a_torn_write_is_not_read_as_consent() {
    let (_dir, path) = scratch();
    grant(&path, terms(), 1).expect("the grant is written");
    let whole = std::fs::read(&path).expect("the record");

    for cut in [1, whole.len() / 3, whole.len() / 2, whole.len() - 2] {
        std::fs::write(&path, &whole[..cut]).expect("the truncated record");
        assert_eq!(
            read(&path),
            Consent::Absent(Reason::Malformed),
            "{cut} bytes of a record is not consent"
        );
    }
}

/// A file of zeros arrives the same way a truncated one does — through the
/// magic field, before any field a caller would act on.
#[test]
fn a_file_of_zeros_is_not_consent() {
    let (_dir, path) = scratch();
    std::fs::write(&path, [0u8; 512]).expect("the zeroed file");
    assert_eq!(read(&path), Consent::Absent(Reason::Malformed));
}

/// Well-formed JSON that is not *this* record. The parse succeeds and the
/// magic is what refuses it.
#[test]
fn another_files_json_is_not_consent() {
    let (_dir, path) = scratch();
    std::fs::write(
        &path,
        br#"{"format":"koru-windows","schema_version":1,"granted":null}"#,
    )
    .expect("the wrong file");
    assert_eq!(read(&path), Consent::Absent(Reason::Malformed));
}

/// A record from a build ahead of this one is not consent **and is not
/// overwritten**, so a downgrade-and-upgrade does not lose an answer.
#[test]
fn a_newer_schema_is_not_consent_and_is_left_alone() {
    let (_dir, path) = scratch();
    let future = format!(
        r#"{{"format":"{FORMAT}","schema_version":{},"granted":null,"future_field":true}}"#,
        SCHEMA_VERSION + 1
    );
    std::fs::write(&path, future.as_bytes()).expect("the future record");

    assert_eq!(read(&path), Consent::Absent(Reason::UnknownSchema));
    assert_eq!(
        std::fs::read_to_string(&path).expect("the record"),
        future,
        "reading must not rewrite what it did not understand"
    );
}

/// Revoking is a recorded answer, not a missing file — the two are different
/// facts and a settings surface has to be able to tell them apart.
#[test]
fn revoking_records_the_answer_rather_than_deleting_it() {
    let (_dir, path) = scratch();
    grant(&path, terms(), 5).expect("the grant");
    revoke(&path).expect("the revocation");

    assert_eq!(read(&path), Consent::Absent(Reason::Revoked));
    assert!(path.is_file(), "the record is still there");
}

/// Granting again after a revocation works, and carries the new terms.
#[test]
fn consent_can_be_given_again() {
    let (_dir, path) = scratch();
    grant(&path, terms(), 5).expect("the grant");
    revoke(&path).expect("the revocation");
    let mut second = terms();
    second.version = "2.0.0".into();
    grant(&path, second.clone(), 9).expect("the second grant");

    let reread = read(&path);
    assert_eq!(reread.granted().map(Granted::terms), Some(&second));
}

/// The write leaves nothing beside the record. A `.conf.new` left lying about
/// is how a later reader meets a file it half believes.
#[test]
fn the_atomic_write_leaves_no_staging_file() {
    let (dir, path) = scratch();
    grant(&path, terms(), 1).expect("the grant");

    let left: Vec<String> = std::fs::read_dir(dir.path())
        .expect("the directory")
        .filter_map(|entry| Some(entry.ok()?.file_name().to_string_lossy().into_owned()))
        .collect();
    assert_eq!(left, vec![CONSENT_FILE.to_owned()]);
}

/// The directory is made rather than demanded: the machine-local directory is
/// created by whichever Koru process needs it first, and voice may be it.
#[test]
fn granting_creates_the_directory() {
    let dir = tempfile::tempdir().expect("a temporary directory");
    let path = path_in(&dir.path().join("koru"));
    grant(&path, terms(), 1).expect("the grant");
    assert!(read(&path).is_granted());
}

#[test]
fn the_same_offer_is_covered() {
    let granted = Granted::recorded(terms(), 1);
    assert!(granted.covers(&terms()));
}

/// A new version under the same licence, no larger, needs no second question.
#[test]
fn a_newer_build_of_the_same_licence_is_covered() {
    let granted = Granted::recorded(terms(), 1);
    let mut offer = terms();
    offer.version = "1.3.0".into();
    offer.download_bytes -= 1024;
    assert!(granted.covers(&offer));
}

#[test]
fn a_different_licence_is_a_new_question() {
    let granted = Granted::recorded(terms(), 1);
    let mut offer = terms();
    offer.licence = "CC-BY-NC-4.0".into();
    assert!(!granted.covers(&offer));
}

#[test]
fn a_bigger_download_is_a_new_question() {
    let granted = Granted::recorded(terms(), 1);
    let mut offer = terms();
    offer.download_bytes += 1;
    assert!(!granted.covers(&offer));
}

/// The figure three surfaces print, rounded once. 648,291,840 bytes is 618.25
/// MB, and a person deciding whether to spend it should be told the larger
/// number.
#[test]
fn megabytes_round_up() {
    assert_eq!(terms().megabytes(), 619);
    assert_eq!(
        Terms {
            download_bytes: 1,
            ..terms()
        }
        .megabytes(),
        1
    );
    assert_eq!(
        Terms {
            download_bytes: 0,
            ..terms()
        }
        .megabytes(),
        0
    );
}

#[test]
fn every_reason_has_a_distinct_token() {
    let all = [
        Reason::NeverAsked,
        Reason::Revoked,
        Reason::Unreadable,
        Reason::Malformed,
        Reason::UnknownSchema,
    ];
    for (at, reason) in all.iter().enumerate() {
        assert!(
            reason
                .as_str()
                .bytes()
                .all(|b| b.is_ascii_lowercase() || b == b'_'),
            "{reason} is not a lowercase token"
        );
        assert!(
            !all[..at]
                .iter()
                .any(|other| other.as_str() == reason.as_str()),
            "{reason} is declared twice"
        );
    }
}
