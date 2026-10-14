// SPDX-License-Identifier: MPL-2.0

//! `ReleaseChannel` had no tests of its own in `koru-update` — the mapping was
//! covered only indirectly, through the update controller. Moving it is the
//! moment to fix that, because these are the properties the store's directory
//! layout now rests on.

use super::*;
use std::str::FromStr;

#[test]
fn a_build_channel_string_maps_to_a_channel() {
    // The pure mapping exists so this can be covered without baking env vars.
    for (given, expected) in [
        ("stable", ReleaseChannel::Stable),
        ("beta", ReleaseChannel::Beta),
        ("nightly", ReleaseChannel::Nightly),
        ("internal", ReleaseChannel::Internal),
        ("dev", ReleaseChannel::Internal),
    ] {
        assert_eq!(
            ReleaseChannel::from_build_channel(given),
            expected,
            "{given}"
        );
    }
}

#[test]
fn case_and_whitespace_do_not_change_the_answer() {
    // The value arrives from an environment variable stamped by a build script;
    // it is not worth failing over a stray newline.
    for given in ["  NIGHTLY  ", "Nightly", "nightly\n"] {
        assert_eq!(
            ReleaseChannel::from_build_channel(given),
            ReleaseChannel::Nightly,
            "{given:?}"
        );
    }
}

#[test]
fn an_unrecognised_build_channel_is_stable() {
    // Deliberately *not* Internal. An unstamped or mistyped channel on a
    // published binary must not quietly grant it the internal-only choices;
    // Stable is the conservative answer.
    for given in ["", "  ", "canary", "release"] {
        assert_eq!(
            ReleaseChannel::from_build_channel(given),
            ReleaseChannel::Stable,
            "{given:?}"
        );
    }
}

#[test]
fn parsing_is_stricter_than_the_build_mapping() {
    // `from_build_channel` cannot fail because a build has *some* channel.
    // `FromStr` reads user input — a preferences file, a command line — where
    // silently substituting Stable would hide a typo.
    assert_eq!(
        ReleaseChannel::from_str("nightly").unwrap(),
        ReleaseChannel::Nightly
    );
    assert!(ReleaseChannel::from_str("canary").is_err());
    let complaint = ReleaseChannel::from_str("canary").unwrap_err();
    assert!(complaint.contains("canary"), "{complaint}");
}

#[test]
fn every_channel_round_trips_through_its_token() {
    for channel in ReleaseChannel::ALL {
        assert_eq!(ReleaseChannel::from_str(channel.as_str()).unwrap(), channel);
        assert_eq!(channel.to_string(), channel.as_str());
        // Stable ASCII: these are path components and CDN path segments.
        assert!(channel.as_str().chars().all(|c| c.is_ascii_lowercase()));
    }
}

#[test]
fn internal_is_the_only_channel_with_no_update_feed() {
    // It is never published, so there is nothing to check against.
    for channel in ReleaseChannel::PUBLIC {
        assert!(channel.has_update_feed(), "{channel}");
    }
    assert!(!ReleaseChannel::Internal.has_update_feed());
}

#[test]
fn public_is_all_without_internal() {
    assert_eq!(ReleaseChannel::ALL.len(), ReleaseChannel::PUBLIC.len() + 1);
    assert!(!ReleaseChannel::PUBLIC.contains(&ReleaseChannel::Internal));
    for channel in ReleaseChannel::PUBLIC {
        assert!(ReleaseChannel::ALL.contains(&channel), "{channel}");
    }
}

#[test]
fn clamping_only_takes_internal_away_from_builds_that_may_not_have_it() {
    // A preferences file carried from an internal build to a published one, or
    // hand-edited, must not leave the install pointed at a channel with no feed.
    for channel in ReleaseChannel::PUBLIC {
        assert_eq!(channel.clamp_to_choosable(), channel, "{channel}");
    }
    let clamped = ReleaseChannel::Internal.clamp_to_choosable();
    if ReleaseChannel::this_build_is_internal() {
        assert_eq!(clamped, ReleaseChannel::Internal);
    } else {
        assert_ne!(clamped, ReleaseChannel::Internal);
        assert!(clamped.has_update_feed());
    }
}

#[test]
fn what_this_build_is_is_one_of_the_four() {
    let mine = ReleaseChannel::of_this_build();
    assert!(ReleaseChannel::ALL.contains(&mine));
    assert_eq!(ReleaseChannel::default(), mine);
    // And the choosable set always contains it, or a build could not pick the
    // channel it already is.
    assert!(ReleaseChannel::choosable().contains(&mine));
}

#[cfg(feature = "serde")]
#[test]
fn serde_uses_the_same_lowercase_token() {
    // The manifest on the CDN is keyed by these strings, so a change here is a
    // change to a published format.
    for channel in ReleaseChannel::ALL {
        let json = serde_json::to_string(&channel).unwrap();
        assert_eq!(json, format!("\"{}\"", channel.as_str()));
        assert_eq!(
            serde_json::from_str::<ReleaseChannel>(&json).unwrap(),
            channel
        );
    }
}
