// SPDX-License-Identifier: MPL-2.0

//! Bounded migration guards. Sources are embedded at compile time; no test I/O.

#[test]
fn migrated_tooltip_consumers_use_the_shared_motion_recipe() {
    for (name, source) in [
        ("theme", include_str!("../theme.rs")),
        ("tooltip", include_str!("../../../koru-ui/src/tooltip.rs")),
        ("editor", include_str!("../../../koru-editor/src/editor.rs")),
        (
            "editor draw",
            include_str!("../../../koru-editor/src/draw.rs"),
        ),
        (
            "document host",
            include_str!("../../../../apps/koru-document/src/lib.rs"),
        ),
        (
            "document bubble",
            include_str!("../../../../apps/koru-document/src/bubble.rs"),
        ),
    ] {
        for retired in [
            "TOOLTIP_DELAY_MS",
            "TOOLTIP_FADE_MS",
            "TOOLTIP_RISE",
            "HOVER_DELAY",
            "APPEAR_DURATION",
            "DISAPPEAR_DURATION",
            "WARM_DURATION",
            "FRAME_OFFSET",
        ] {
            assert!(
                !source.contains(retired),
                "{name}: {retired} is retired; use theme::motion::tooltip() fields"
            );
        }
        if name == "tooltip" {
            for literal in [
                "Duration::from_millis(400)",
                "Duration::from_millis(100)",
                "Duration::from_millis(75)",
                "Duration::from_millis(300)",
            ] {
                assert!(
                    !source.contains(literal),
                    "tooltip: use theme::motion::tooltip() instead of {literal}"
                );
            }
        }
    }
}

#[test]
fn anchored_popover_keeps_geometry_in_the_overlay_recipe() {
    let source = include_str!("../../../koru-ui/src/popover.rs");
    for retired in ["const EDGE:", "const GAP:", "radius: 10.0"] {
        assert!(
            !source.contains(retired),
            "popover: use theme::overlay::popover() instead of {retired}"
        );
    }
}
