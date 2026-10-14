// SPDX-License-Identifier: MPL-2.0

use super::*;

#[test]
fn the_header_line_carries_every_field() {
    // A run whose header is incomplete is a run whose numbers cannot be
    // compared, which is why every field prints even when it is unknown.
    let facts = HostFacts::detect();
    let line = facts.line();
    for field in [&facts.os, &facts.arch, &facts.cores, &facts.simd] {
        assert!(!field.is_empty());
        assert!(line.contains(field.as_str()), "{line} is missing {field}");
    }
}

#[test]
fn peak_rss_is_a_plausible_number_on_a_platform_that_answers() {
    // Not "greater than zero" as a tautology: the point is that the counter is
    // the OS's own peak rather than a sample, so it must already be non-trivial
    // for a process that has loaded a test harness.
    if let Some(bytes) = peak_rss_bytes() {
        assert!(bytes > 1_000_000, "{bytes} bytes is not a process");
    }
}

#[test]
fn cpu_time_only_goes_up() {
    let Some(before) = cpu_time() else {
        return;
    };
    // Do some arithmetic the optimiser cannot remove.
    let mut acc = 0u64;
    for i in 0..2_000_000u64 {
        acc = acc.wrapping_add(i ^ acc);
    }
    assert_ne!(acc, 1);
    let after = cpu_time().expect("the platform answered once already");
    assert!(after >= before, "{after:?} < {before:?}");
}

#[test]
fn a_percentage_over_a_zero_interval_is_absent_rather_than_zero() {
    assert_eq!(
        cpu_percent(
            Some(Duration::from_millis(1)),
            Some(Duration::from_millis(2)),
            Duration::ZERO
        ),
        None
    );
    assert_eq!(
        cpu_percent(None, Some(Duration::from_millis(2)), Duration::from_secs(1)),
        None
    );
}

#[test]
fn one_saturated_core_reads_a_hundred() {
    let percent = cpu_percent(
        Some(Duration::ZERO),
        Some(Duration::from_secs(1)),
        Duration::from_secs(1),
    )
    .expect("both endpoints");
    assert!((percent - 100.0).abs() < 1e-9, "{percent}");

    // And two saturated cores read 200, which is what makes the figure
    // comparable against a declared thread count.
    let two = cpu_percent(
        Some(Duration::ZERO),
        Some(Duration::from_secs(2)),
        Duration::from_secs(1),
    )
    .expect("both endpoints");
    assert!((two - 200.0).abs() < 1e-9, "{two}");
}
