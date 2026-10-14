// SPDX-License-Identifier: MPL-2.0

use super::*;
use crate::vectors;
use koru_account_core::workspace_authorization::Region;
use koru_account_session::cloud::blob::BlobPurpose;

fn claims() -> BlobCapabilityClaimsV1 {
    BlobCapabilityClaimsV1 {
        v: 1,
        pur: BlobPurpose::Write.as_str().into(),
        iss: "https://us.example.invalid".into(),
        aud: "us.data.koru.do".into(),
        rgn: Region::Us,
        acc: vectors::fixture_id("acc-1"),
        wsp: vectors::fixture_id("ws-1"),
        inc: vectors::fixture_id("inc-1"),
        eph: 7,
        sub: vectors::fixture_id("upload-1"),
        loc: vectors::fixture_id("loc-1"),
        bh: vectors::fixture_hash("b3:aa"),
        len: 4_194_304,
        nbf: 1_800_000_000,
        exp: 1_800_000_300,
        jti: vectors::fixture_id("jti-1"),
    }
}

/// The four bindings `cases.json` names, each from its own row, plus the world
/// they agree on. The order matters and is asserted by the codes: addressing
/// first, so a caller that named another object is never told about an epoch.
#[test]
fn the_four_binding_vectors_decide_as_the_contract_says() {
    let mut seen = 0;
    for case in vectors::cases() {
        let Some(expected) = (match case.id.as_str() {
            "C12" => Some(BlobCode::WrongObject),
            "C13" => Some(BlobCode::WrongWorkspace),
            "C14" => Some(BlobCode::StaleIncarnation),
            "C15" => Some(BlobCode::StaleEpoch),
            _ => None,
        }) else {
            continue;
        };
        seen += 1;
        assert_eq!(case.expected(), Some(expected), "{}", case.id);
        let world = BindingWorld {
            route_subject: Some(vectors::fixture_id(
                case.route_sub.as_deref().unwrap_or("upload-1"),
            )),
            addressed_workspace: Some(vectors::fixture_id(
                case.session_wsp.as_deref().unwrap_or("ws-1"),
            )),
            current_incarnation: Some(vectors::fixture_id(
                case.current_inc.as_deref().unwrap_or("inc-1"),
            )),
            current_access_epoch: Some(case.current_eph.unwrap_or(7)),
            session_terminal: false,
        };
        assert_eq!(
            check_bindings(&claims(), &world).unwrap_err(),
            expected,
            "{} — {}",
            case.id,
            case.note
        );
    }
    assert_eq!(seen, 4, "every binding row of cases.json was exercised");
}

/// A world that agrees admits, and the worker's world — which knows only the id
/// in the URL — admits on that alone rather than assuming the rest matches.
#[test]
fn an_unknown_fact_is_not_compared_and_is_never_assumed_to_match() {
    let agreeing = BindingWorld {
        route_subject: Some(vectors::fixture_id("upload-1")),
        addressed_workspace: Some(vectors::fixture_id("ws-1")),
        current_incarnation: Some(vectors::fixture_id("inc-1")),
        current_access_epoch: Some(7),
        session_terminal: false,
    };
    assert!(check_bindings(&claims(), &agreeing).is_ok());

    // What the object worker actually holds: no account database at all.
    let worker = BindingWorld {
        route_subject: Some(vectors::fixture_id("upload-1")),
        ..BindingWorld::default()
    };
    assert!(check_bindings(&claims(), &worker).is_ok());
    let wrong = BindingWorld {
        route_subject: Some(vectors::fixture_id("upload-2")),
        ..BindingWorld::default()
    };
    assert_eq!(
        check_bindings(&claims(), &wrong).unwrap_err(),
        BlobCode::WrongObject
    );
}

/// A terminal session refuses whatever the capability still says, which is what
/// makes an abort a fence rather than a request to stop trying.
#[test]
fn a_terminal_session_refuses_a_capability_that_is_still_valid() {
    let world = BindingWorld {
        route_subject: Some(vectors::fixture_id("upload-1")),
        session_terminal: true,
        ..BindingWorld::default()
    };
    assert_eq!(
        check_bindings(&claims(), &world).unwrap_err(),
        BlobCode::UploadExpired
    );
}

/// C16: an account bearer token sent to a byte route fails as a malformed
/// credential rather than being tried as the other kind.
#[test]
fn a_bearer_token_at_a_byte_route_is_a_malformed_credential() {
    assert_eq!(credential(Some("Capability abc"), "Capability"), Ok("abc"));
    for header in [
        Some("Bearer abc"),
        Some("capability abc"),
        Some("CAPABILITY abc"),
        Some("Capability"),
        Some("Capability "),
        Some("Capability a b"),
        Some("abc"),
        Some(""),
        None,
    ] {
        assert_eq!(
            credential(header, "Capability").unwrap_err(),
            BlobCode::CapabilityInvalid,
            "{header:?}"
        );
    }
    // And the worker's own service credential is a third distinct word.
    assert_eq!(credential(Some("Worker abc"), "Worker"), Ok("abc"));
    assert!(credential(Some("Capability abc"), "Worker").is_err());
}
