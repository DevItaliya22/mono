// SPDX-License-Identifier: MPL-2.0

use super::*;

fn digest(byte: u8) -> KeyedDigest {
    KeyedDigest(vec![byte; 32])
}

fn state() -> ChallengeState {
    ChallengeState {
        address_digest: digest(7),
        code_digest: digest(1),
        attempts: 0,
        last_sent_at_millis: 1_000,
        expires_at_millis: 1_000 + CODE_LIFETIME_MILLIS,
        consumed: false,
    }
}

#[test]
fn an_address_normalizes_the_way_the_google_path_normalizes() {
    // The whole point: the provider path (`resolver.rs`) stores
    // `email.trim().to_lowercase()`, and `classify()` matches on that string.
    // If these two ever disagree, one person gets two accounts.
    for raw in [
        "  Alice@Example.COM ",
        "alice@example.com",
        "ALICE@EXAMPLE.COM",
    ] {
        assert_eq!(
            normalize_address(raw).expect("valid").as_str(),
            raw.trim().to_lowercase(),
        );
    }
}

#[test]
fn normalization_does_not_fold_addresses_a_provider_keeps_apart() {
    // Stripping +tags or dots would map two deliverable addresses onto one
    // credential, and which of them a provider treats as equal differs by
    // provider. Koru does not guess.
    let tagged = normalize_address("alice+koru@example.com").expect("valid");
    let plain = normalize_address("alice@example.com").expect("valid");
    assert_ne!(tagged.as_str(), plain.as_str());

    let dotted = normalize_address("a.lice@example.com").expect("valid");
    assert_ne!(dotted.as_str(), plain.as_str());
}

#[test]
fn what_cannot_be_an_address_is_refused() {
    assert_eq!(normalize_address("   "), Err(AddressRejection::Empty));
    assert_eq!(normalize_address("alice"), Err(AddressRejection::Shape));
    assert_eq!(
        normalize_address("@example.com"),
        Err(AddressRejection::Shape)
    );
    assert_eq!(normalize_address("alice@"), Err(AddressRejection::Shape));
    assert_eq!(
        normalize_address("alice@ex@ample.com"),
        Err(AddressRejection::Shape)
    );
    assert_eq!(
        normalize_address("alice@localhost"),
        Err(AddressRejection::Shape)
    );
    assert_eq!(
        normalize_address("alice@.example.com"),
        Err(AddressRejection::Shape)
    );
    assert_eq!(
        normalize_address("ali ce@example.com"),
        Err(AddressRejection::Shape)
    );
    let long = format!("{}@example.com", "a".repeat(MAX_ADDRESS_BYTES));
    assert_eq!(normalize_address(&long), Err(AddressRejection::TooLong));
}

#[test]
fn a_deliverable_oddity_is_accepted_because_the_code_is_the_real_validator() {
    // An address only becomes an account after a code sent to it comes back,
    // so refusing a technically-valid address costs more than accepting an
    // undeliverable one.
    assert!(normalize_address("a@b.co").is_ok());
    assert!(normalize_address("first.last+tag@sub.domain.example").is_ok());
    assert!(normalize_address("试@例子.测试").is_ok());
}

#[test]
fn a_code_is_six_digits_and_survives_being_pasted() {
    assert_eq!(
        VerificationCode::from_input("123456").unwrap().as_str(),
        "123456"
    );
    // Mail clients break codes up; people paste what they see.
    assert_eq!(
        VerificationCode::from_input("123 456").unwrap().as_str(),
        "123456"
    );
    assert_eq!(
        VerificationCode::from_input("123-456").unwrap().as_str(),
        "123456"
    );
    assert_eq!(
        VerificationCode::from_input(" 123456 ").unwrap().as_str(),
        "123456"
    );

    assert!(VerificationCode::from_input("12345").is_none());
    assert!(VerificationCode::from_input("1234567").is_none());
    assert!(VerificationCode::from_input("12345a").is_none());
    assert!(VerificationCode::from_input("").is_none());
}

#[test]
fn a_generated_code_is_always_six_digits_including_leading_zeros() {
    struct Fixed(u64);
    impl Entropy for Fixed {
        fn fill(&self, output: &mut [u8]) -> Result<(), CryptoError> {
            output.copy_from_slice(&self.0.to_le_bytes());
            Ok(())
        }
    }
    // A small value must not render as "7" -- zero padding is what keeps every
    // code the same length for the person typing it and for the digest.
    assert_eq!(generate_code(&Fixed(7)).unwrap().as_str(), "000007");
    assert_eq!(generate_code(&Fixed(0)).unwrap().as_str(), "000000");
    assert_eq!(generate_code(&Fixed(999_999)).unwrap().as_str(), "999999");
    // And it wraps into range rather than overflowing the width.
    assert_eq!(generate_code(&Fixed(1_000_000)).unwrap().as_str(), "000000");
    assert_eq!(generate_code(&Fixed(u64::MAX)).unwrap().as_str().len(), 6);
}

#[test]
fn the_right_code_is_accepted_and_a_wrong_one_counts_down() {
    let state = state();
    assert_eq!(
        verify_code(&state, &digest(1), 2_000),
        CodeVerdict::Accepted
    );
    assert_eq!(
        verify_code(&state, &digest(9), 2_000),
        CodeVerdict::Wrong { attempts_left: 4 }
    );
}

#[test]
fn the_fifth_wrong_guess_kills_the_challenge() {
    let mut state = state();
    for attempts in 0..MAX_CODE_ATTEMPTS {
        state.attempts = attempts;
        let verdict = verify_code(&state, &digest(9), 2_000);
        if attempts == MAX_CODE_ATTEMPTS - 1 {
            assert_eq!(verdict, CodeVerdict::Exhausted, "at attempt {attempts}");
        } else {
            assert_eq!(
                verdict,
                CodeVerdict::Wrong {
                    attempts_left: MAX_CODE_ATTEMPTS - attempts - 1
                }
            );
        }
    }
}

#[test]
fn past_the_bound_even_the_right_code_is_refused() {
    // Otherwise the bound is a speed bump: a correct guess on the sixth try
    // would still open the account.
    let mut state = state();
    state.attempts = MAX_CODE_ATTEMPTS;
    assert_eq!(
        verify_code(&state, &digest(1), 2_000),
        CodeVerdict::Exhausted
    );
}

#[test]
fn expiry_and_replay_are_judged_before_the_code_is_compared() {
    // A stale or replayed code must not be able to spend an attempt, and must
    // not report a different verdict for a right guess than for a wrong one.
    let mut expired = state();
    expired.expires_at_millis = 1_500;
    assert_eq!(
        verify_code(&expired, &digest(1), 1_500),
        CodeVerdict::Expired
    );
    assert_eq!(
        verify_code(&expired, &digest(9), 1_500),
        CodeVerdict::Expired
    );

    let mut consumed = state();
    consumed.consumed = true;
    assert_eq!(
        verify_code(&consumed, &digest(1), 2_000),
        CodeVerdict::AlreadyConsumed
    );
    assert_eq!(
        verify_code(&consumed, &digest(9), 2_000),
        CodeVerdict::AlreadyConsumed
    );
}

#[test]
fn expiry_is_exclusive_at_the_boundary_millisecond() {
    let state = state();
    let last_valid = state.expires_at_millis - 1;
    assert_eq!(
        verify_code(&state, &digest(1), last_valid),
        CodeVerdict::Accepted
    );
    assert_eq!(
        verify_code(&state, &digest(1), state.expires_at_millis),
        CodeVerdict::Expired
    );
}

#[test]
fn a_resend_waits_out_the_cooldown() {
    let state = state();
    assert_eq!(
        resend_decision(&state, state.last_sent_at_millis),
        ResendVerdict::TooSoon {
            retry_after_millis: RESEND_COOLDOWN_MILLIS
        }
    );
    assert_eq!(
        resend_decision(
            &state,
            state.last_sent_at_millis + RESEND_COOLDOWN_MILLIS - 1
        ),
        ResendVerdict::TooSoon {
            retry_after_millis: 1
        }
    );
    assert_eq!(
        resend_decision(&state, state.last_sent_at_millis + RESEND_COOLDOWN_MILLIS),
        ResendVerdict::Send
    );
}

#[test]
fn nothing_here_prints_an_address_or_a_code() {
    let address = normalize_address("alice@example.com").expect("valid");
    assert_eq!(format!("{address:?}"), "NormalizedAddress(<redacted>)");
    let code = VerificationCode::from_input("123456").expect("valid");
    assert_eq!(format!("{code:?}"), "VerificationCode(<redacted>)");
    let mail = OutboundMail {
        to: address,
        subject: "s".into(),
        text: "your code is 123456".into(),
        html: "<p>123456</p>".into(),
    };
    let rendered = format!("{mail:?}");
    assert_eq!(rendered, "OutboundMail(<redacted>)");
    assert!(!rendered.contains("123456"));
}
