// SPDX-License-Identifier: MPL-2.0

//! The layout pass, asserted back to itself.

use super::*;

#[test]
fn a_chip_row_wraps_at_the_column_edge_and_every_control_is_where_it_is_drawn() {
    let mut engine = TextEngine::new();
    let mut column = Column::new(&mut engine, 10.0, 200.0, 0.0);
    column.chips([
        (
            Action::TakeControl,
            Face::Chip("a very wide label indeed".into()),
            true,
        ),
        (
            Action::TakeControlNow,
            Face::Chip("another wide label".into()),
            true,
        ),
    ]);
    let laid = column.finish();
    assert_eq!(laid.controls.len(), 2);
    // The second wrapped rather than running past the right edge.
    assert!(laid.controls[1].rect.y > laid.controls[0].rect.y);
    for control in &laid.controls {
        assert!(
            control.rect.x >= 10.0 && control.rect.x + control.rect.w <= 10.0 + 200.0 + CHIP_MIN_W
        );
        assert!(
            control
                .rect
                .contains(control.rect.x + 1.0, control.rect.y + 1.0)
        );
    }
}

#[test]
fn a_consequence_takes_room_under_the_control_it_belongs_to() {
    let mut engine = TextEngine::new();
    let mut column = Column::new(&mut engine, 0.0, 300.0, 0.0);
    column.chips([(
        Action::AskToEndRun,
        Face::Danger("End the run".into()),
        true,
    )]);
    let before = column.finish();
    let mut engine = TextEngine::new();
    let mut column = Column::new(&mut engine, 0.0, 300.0, 0.0);
    column.chips([(
        Action::AskToEndRun,
        Face::Danger("End the run".into()),
        true,
    )]);
    column.consequence("The run stops for good.");
    let after = column.finish();
    assert!(
        after.bottom > before.bottom,
        "the sentence is laid out, not hovered"
    );
    assert_eq!(after.paints.len(), 1);
}

/// **A rating chip reserves exactly the icon it draws.**
///
/// The thumbs were laid out through `simple_layout` in the *body* role,
/// which has no glyph at a Phosphor codepoint, so the chips drew a
/// missing-glyph box — what the maintainers saw under *How did it go?*. The mark
/// now goes through `icon_layout`, and the width the row measures and the
/// offset the label is drawn at are the same two numbers rather than a
/// reserved `18.0` and whatever the laid-out glyph happened to be.
#[test]
fn a_rating_chip_reserves_exactly_the_icon_it_draws() {
    let width_of = |face: Face| {
        let mut engine = TextEngine::new();
        let mut column = Column::new(&mut engine, 0.0, 600.0, 0.0);
        column.chips([(Action::Rate(koru_delegate_core::Rating::Good), face, true)]);
        column.finish().controls[0].rect.w
    };
    let label = "Good, and it did what I asked";
    let plain = width_of(Face::Chip(label.into()));
    let rated = width_of(Face::Rating(
        label.into(),
        koru_draw::icons::THUMBS_UP,
        false,
    ));
    assert!(
        plain > CHIP_MIN_W,
        "the label has to be wide enough that neither chip is clamped to the minimum"
    );
    assert_eq!(
        rated - plain,
        chip_icon_size() + CHIP_ICON_GAP,
        "the room made for the mark is the room the mark is drawn in"
    );
}
