// SPDX-License-Identifier: MPL-2.0

use super::*;

/// The token is what a signed descriptor carries and what `preferences.conf`
/// stores, so serde and `as_str` may never come apart.
#[test]
fn serde_writes_the_stable_token() {
    for tier in ModelTier::ALL {
        assert_eq!(
            serde_json::to_string(&tier).unwrap(),
            format!("\"{}\"", tier.as_str())
        );
        assert_eq!(ModelTier::parse(tier.as_str()), Some(tier));
    }
}

/// An unknown tier is `None`, not `Compact`. Defaulting would install a model
/// nobody asked for while looking like it worked.
#[test]
fn an_unknown_tier_is_refused_rather_than_defaulted() {
    assert_eq!(ModelTier::parse("HighAccuracy"), None);
    assert_eq!(ModelTier::parse("balanced"), None);
    assert_eq!(ModelTier::parse(""), None);
}

/// The first dictation must not wait on the largest download in the product.
#[test]
fn the_default_is_the_cheap_one() {
    assert_eq!(ModelTier::default(), ModelTier::Compact);
    assert_eq!(ModelTier::ALL[0], ModelTier::default());
}
