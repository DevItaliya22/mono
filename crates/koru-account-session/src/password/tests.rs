// SPDX-License-Identifier: MPL-2.0

use super::*;
use koru_account_core::check_password;

fn hashing() -> Argon2PasswordHashing {
    Argon2PasswordHashing::new().expect("argon2 parameters are valid")
}

#[test]
fn a_password_verifies_against_its_own_verifier() {
    let hashing = hashing();
    let accepted = check_password("Passw0rd!").expect("policy");
    let verifier = hashing.hash(&accepted).expect("hash");
    assert_eq!(
        hashing
            .verify("Passw0rd!", Some(&verifier))
            .expect("verify"),
        PasswordCheck::Accepted { rehash: false }
    );
    assert_eq!(
        hashing
            .verify("Passw0rd?", Some(&verifier))
            .expect("verify"),
        PasswordCheck::Rejected
    );
}

#[test]
fn two_verifiers_for_one_password_differ() {
    // Salted: the same password hashed twice must not produce the same string,
    // or one stolen verifier would identify everybody who chose that password.
    let hashing = hashing();
    let first = hashing
        .hash(&check_password("Passw0rd!").expect("policy"))
        .expect("hash");
    let second = hashing
        .hash(&check_password("Passw0rd!").expect("policy"))
        .expect("hash");
    assert_ne!(first.0, second.0);
    assert_eq!(
        hashing.verify("Passw0rd!", Some(&first)).expect("verify"),
        PasswordCheck::Accepted { rehash: false }
    );
    assert_eq!(
        hashing.verify("Passw0rd!", Some(&second)).expect("verify"),
        PasswordCheck::Accepted { rehash: false }
    );
}

#[test]
fn the_stored_verifier_carries_its_own_parameters() {
    // This is what makes rehash-on-sign-in possible without a migration.
    let hashing = hashing();
    let verifier = hashing
        .hash(&check_password("Passw0rd!").expect("policy"))
        .expect("hash");
    assert!(verifier.0.starts_with("$argon2id$"));
    assert!(verifier.0.contains(&format!("m={MEMORY_KIB}")));
    assert!(verifier.0.contains(&format!("t={ITERATIONS}")));
    assert!(verifier.0.contains(&format!("p={LANES}")));
}

#[test]
fn a_verifier_made_with_weaker_parameters_asks_to_be_rehashed() {
    // The promise of §5.2: raising the cost later costs one write on each
    // person's next sign-in, not a migration and not a forced reset.
    let weaker = Argon2::new(
        Algorithm::Argon2id,
        Version::V0x13,
        Params::new(MEMORY_KIB / 2, 1, LANES, None).expect("params"),
    );
    let stored = PasswordVerifier(
        weaker
            .hash_password(b"Passw0rd!", &salt())
            .expect("hash")
            .to_string(),
    );
    assert_eq!(
        hashing()
            .verify("Passw0rd!", Some(&stored))
            .expect("verify"),
        PasswordCheck::Accepted { rehash: true }
    );
}

#[test]
fn an_absent_account_is_rejected_and_never_accepted() {
    let hashing = hashing();
    assert_eq!(
        hashing.verify("Passw0rd!", None).expect("verify"),
        PasswordCheck::Rejected
    );
    // Even the placeholder itself must not authenticate anything.
    assert_eq!(
        hashing.verify(ABSENT_PASSWORD, None).expect("verify"),
        PasswordCheck::Rejected
    );
}

#[test]
fn an_absent_account_costs_what_a_present_one_costs() {
    // The timing-oracle defence. A verification costs ~10 ms here (see
    // `measure_verification_cost`), so a path that skipped the hash entirely
    // would stand out by orders of magnitude. The assertion is a ratio rather
    // than an absolute time, which would flake on a loaded machine.
    use std::time::Instant;

    let hashing = hashing();
    let verifier = hashing
        .hash(&check_password("Passw0rd!").expect("policy"))
        .expect("hash");

    // One of each first, so neither timing pays a cold-cache cost.
    let _ = hashing.verify("Wrongpass1!", Some(&verifier));
    let _ = hashing.verify("Wrongpass1!", None);

    let started = Instant::now();
    let _ = hashing.verify("Wrongpass1!", Some(&verifier));
    let present = started.elapsed();

    let started = Instant::now();
    let _ = hashing.verify("Wrongpass1!", None);
    let absent = started.elapsed();

    let ratio = absent.as_secs_f64() / present.as_secs_f64().max(f64::EPSILON);
    assert!(
        (0.5..=2.0).contains(&ratio),
        "absent {absent:?} and present {present:?} must cost alike; ratio {ratio}"
    );
}

#[test]
#[ignore = "measurement, not an assertion: run with --ignored --nocapture"]
fn measure_verification_cost() {
    use std::time::Instant;
    let hashing = hashing();
    let verifier = hashing
        .hash(&check_password("Passw0rd!").expect("policy"))
        .expect("hash");
    let _ = hashing.verify("Passw0rd!", Some(&verifier));
    let runs = 20;
    let started = Instant::now();
    for _ in 0..runs {
        let _ = hashing.verify("Passw0rd!", Some(&verifier));
    }
    let each = started.elapsed() / runs;
    println!("argon2id m={MEMORY_KIB}KiB t={ITERATIONS} p={LANES}: {each:?} per verification");
}
