// SPDX-License-Identifier: MPL-2.0

use super::*;

fn rejection(raw: &str) -> PasswordRejection {
    check_password(raw).err().expect("expected a rejection")
}

fn accepted(raw: &str) -> String {
    check_password(raw)
        .unwrap_or_else(|error| panic!("expected {raw:?} to be accepted, got {error:?}"))
        .expose()
        .to_owned()
}

#[test]
fn three_of_four_classes_is_enough() {
    // upper + lower + digit, no symbol.
    accepted("Passw0rdy");
    // lower + digit + symbol, no uppercase.
    accepted("passw0rd!");
    // upper + lower + symbol, no digit.
    accepted("Password!");
}

#[test]
fn two_classes_is_not_enough() {
    let PasswordRejection::TooFewClasses { present, required } = rejection("passwordy") else {
        panic!("expected a class rejection");
    };
    assert_eq!(present, vec![CharacterClass::Lowercase]);
    assert_eq!(required, REQUIRED_CHARACTER_CLASSES);

    let PasswordRejection::TooFewClasses { present, .. } = rejection("Passwordy") else {
        panic!("expected a class rejection");
    };
    assert_eq!(
        present,
        vec![CharacterClass::Uppercase, CharacterClass::Lowercase]
    );
}

#[test]
fn a_space_is_a_symbol_so_a_passphrase_qualifies() {
    // Three classes only because the space counts: the point of defining the
    // fourth class as "not a letter or a digit" rather than as a punctuation
    // list.
    accepted("Correct horse battery");
    assert!(classes_present("a b").contains(&CharacterClass::Symbol));
}

#[test]
fn a_non_latin_password_is_not_held_to_a_latin_rule() {
    // Japanese characters are neither upper nor lower nor numeric, so they are
    // symbols; with digits and a Latin letter this reaches three classes.
    accepted("パスワード12a");

    // And length is counted in characters, not bytes. This one is 7 characters
    // -- one short -- but 19 bytes, so a byte-based bound would have let it
    // through: the same policy would be strict in English and lax in Japanese.
    let seven = "パスワード1a";
    assert_eq!(seven.chars().count(), 7);
    assert!(seven.len() > MIN_PASSWORD_CHARS);
    assert!(matches!(
        rejection(seven),
        PasswordRejection::TooShort { characters: 7, .. }
    ));
}

#[test]
fn length_boundaries_are_inclusive_at_the_minimum_and_the_maximum() {
    assert!(matches!(
        rejection("Pas5wor"),
        PasswordRejection::TooShort {
            characters: 7,
            required: 8
        }
    ));
    accepted("Pas5word");

    let at_limit: String = std::iter::repeat_n('a', MAX_PASSWORD_CHARS - 2)
        .chain(['A', '1'])
        .collect();
    assert_eq!(accepted(&at_limit).chars().count(), MAX_PASSWORD_CHARS);

    let over: String = std::iter::repeat_n('a', MAX_PASSWORD_CHARS - 1)
        .chain(['A', '1'])
        .collect();
    assert!(matches!(
        rejection(&over),
        PasswordRejection::TooLong {
            characters: 129,
            allowed: 128
        }
    ));
}

#[test]
fn the_two_spellings_of_one_accented_password_normalize_to_one() {
    // "Café1234": composed é, then e + U+0301.
    let composed = "Caf\u{e9}1234";
    let decomposed = "Cafe\u{301}1234";
    assert_ne!(composed, decomposed);
    assert_eq!(accepted(composed), accepted(decomposed));
}

#[test]
fn normalization_happens_before_the_length_is_counted() {
    // Seven composed characters, spelled decomposed as eight code points. If
    // the count ran first, the decomposed spelling would pass a policy the
    // composed spelling fails -- the same password, two verdicts.
    let decomposed = "Cafe\u{301}12";
    assert_eq!(decomposed.chars().count(), 7);
    assert!(matches!(
        rejection(decomposed),
        PasswordRejection::TooShort { characters: 6, .. }
    ));
}

#[test]
fn normalize_password_leaves_an_already_normal_password_alone() {
    let plain = "Passw0rd!";
    assert_eq!(normalize_password(plain), plain);
}
