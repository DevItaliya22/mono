// SPDX-License-Identifier: MPL-2.0

use super::*;

/// The sibling this file is a copy of. Read at compile time so the two bodies
/// cannot silently diverge: they are maintained by hand, and a hand needs
/// something that tells it when it forgot.
///
/// **It compares the arithmetic, not the prose.** The two modules document
/// themselves differently on purpose — one explains the policy, the other
/// explains the duplication — so what is checked is the body of `push`, which
/// is the part that has to agree.
const SIBLING: &str = include_str!("../../../koru-voice-sherpa/src/redecode.rs");
const MINE: &str = include_str!("../redecode.rs");

/// Everything between `fn push` and the closing of its block, whitespace
/// collapsed.
fn push_body(source: &str) -> String {
    let start = source
        .find("pub fn push(&mut self, samples: usize) -> bool {")
        .expect("both copies declare push with this signature");
    let tail = &source[start..];
    let end = tail.find("\n    }").expect("push has a closing brace");
    tail[..end]
        .lines()
        // Comment *lines* go, whole. Filtering tokens instead would keep the
        // prose after the `//` and make every difference in wording a failure,
        // which is the opposite of what this checks.
        .filter(|line| !line.trim_start().starts_with("//"))
        .flat_map(str::split_whitespace)
        .collect::<Vec<_>>()
        .join(" ")
}

#[test]
fn this_copy_and_the_sherpa_one_count_identically() {
    // The whole justification for two copies is that the arithmetic is settled
    // and nobody tunes it. This is what makes that claim checkable rather than
    // a hope. If it fails, the fix is to make the two bodies the same again —
    // or to collapse them, which Q-V49 says how to do.
    let mine = push_body(MINE);
    let theirs = push_body(SIBLING);
    assert_eq!(
        mine, theirs,
        "koru-voice-whisper's Redecode::push has drifted from koru-voice-sherpa's"
    );
}

const FRAME: usize = 320;

#[test]
fn a_500ms_interval_fires_every_twenty_five_frames() {
    let mut counter = Redecode::every(Duration::from_millis(500));
    assert_eq!(counter.interval_samples(), 8_000);
    let mut fires = 0;
    for _ in 0..100 {
        if counter.push(FRAME) {
            fires += 1;
        }
    }
    assert_eq!(fires, 4);
    assert_eq!(counter.elapsed_audio(), Duration::from_secs(2));
}

#[test]
fn one_push_carrying_several_intervals_is_one_redecode() {
    let mut counter = Redecode::every(Duration::from_millis(500));
    assert!(counter.push(16_000 * 5));
    assert!(!counter.push(FRAME));
}

#[test]
fn a_disabled_counter_never_fires_but_still_counts_audio() {
    let mut counter = Redecode::never();
    assert!(!counter.is_enabled());
    for _ in 0..50 {
        assert!(!counter.push(FRAME));
    }
    assert_eq!(counter.total_samples(), 16_000);
}
