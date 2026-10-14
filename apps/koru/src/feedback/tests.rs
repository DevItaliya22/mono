// SPDX-License-Identifier: MPL-2.0

use super::*;

fn view() -> FeedbackView {
    draft::reset();
    FeedbackView::new()
}

#[test]
fn send_stays_disabled_until_there_are_a_few_words() {
    let mut v = view();
    assert!(!v.can_send_now());
    v.set_message("The tab would not close");
    assert!(v.can_send_now());
}

#[test]
fn switching_kind_keeps_the_message() {
    let mut v = view();
    v.set_message("A faster search");
    v.click_kind(Kind::Idea);
    assert_eq!(v.kind(), Kind::Idea);
    assert_eq!(v.message(), "A faster search");
}

#[test]
fn contact_requires_an_email_before_send() {
    let mut v = view();
    v.set_message("Something broke");
    v.toggle_contact();
    assert!(v.contact());
    assert!(!v.can_send_now());
    v.set_email("not-valid");
    assert!(!v.can_send_now());
    v.set_email("person@koru.do");
    assert!(v.can_send_now());
}

#[test]
fn closing_and_reopening_restores_the_draft_until_a_successful_send() {
    {
        let mut v = view();
        v.set_message("Still happening");
        v.click_kind(Kind::General);
        v.attach_bytes("shot.png", &submit::tiny_png());
        drop(v);
    }
    {
        let v = FeedbackView::new();
        assert_eq!(v.kind(), Kind::General);
        assert_eq!(v.message(), "Still happening");
        assert_eq!(v.attachment_count(), 1);
        drop(v);
    }
    {
        let mut v = FeedbackView::new();
        v.forget = true;
        draft::clear();
        drop(v);
    }
    let v = FeedbackView::new();
    assert!(v.message().is_empty());
}

#[test]
fn attaching_more_than_three_images_is_refused() {
    let mut v = view();
    for i in 0..3 {
        v.attach_bytes(&format!("shot{i}.png"), &submit::tiny_png());
    }
    assert_eq!(v.attachment_count(), 3);
    v.attach_bytes("shot3.png", &submit::tiny_png());
    assert_eq!(v.attachment_count(), 3);
}

#[test]
fn kind_cards_use_colour_emoji() {
    assert_eq!(Kind::Bug.emoji(), "\u{1F41B}");
    assert_eq!(Kind::Idea.emoji(), "\u{1F4A1}");
    assert_eq!(Kind::General.emoji(), "\u{1F4AC}");
}

#[test]
fn enter_in_the_composer_inserts_a_newline() {
    let mut v = view();
    v.set_message("hello");
    v.press_enter();
    assert_eq!(v.raw_message(), "hello\n");
}

#[test]
fn importance_options_share_one_row_height() {
    let mut v = view();
    v.click_kind(Kind::Idea);
    let laid = v.form_layout();
    assert_eq!(laid.importances.len(), 4);
    let h = laid.importances[0].1.h;
    assert!(h >= theme::role::row().metrics.line_height);
    assert!(laid.importances.iter().all(|(_, rect)| rect.h == h));
}

#[test]
fn frequency_chips_are_measured_not_fixed_width() {
    let mut v = view();
    v.click_kind(Kind::Bug);
    let laid = v.form_layout();
    assert_eq!(laid.freqs.len(), 3);
    assert!(
        laid.freqs
            .iter()
            .all(|(_, rect)| rect.h == koru_ui::Chip::H)
    );
    let once = laid.freqs[0].1.w;
    let every = laid.freqs[2].1.w;
    assert!(once >= koru_ui::chip::MIN_W);
    assert!(every > once, "Every time is wider than Once");
}

#[test]
fn screenshot_filename_sits_below_the_thumb() {
    let mut v = view();
    v.attach_bytes("a-very-long-screenshot-name.png", &submit::tiny_png());
    let laid = v.form_layout();
    assert_eq!(laid.shots.len(), 1);
    let shot = &laid.shots[0];
    assert!(
        shot.name.y >= shot.thumb.y + shot.thumb.h,
        "the name is below the image, not inside it"
    );
    assert!(shot.close.x >= shot.thumb.x);
    assert!(shot.close.x + shot.close.w <= shot.thumb.x + shot.thumb.w + 0.5);
}

#[test]
fn cancel_and_send_sit_inside_the_content_column() {
    let mut v = view();
    let laid = v.form_layout();
    // The composer spans the content column, so its right edge is the column's.
    let right = laid.composer.x + laid.composer.w;
    assert!(laid.cancel.x >= laid.x);
    assert!(laid.send.x + laid.send.w <= right + 0.5);
    assert!(
        laid.send.y >= laid.composer.y + laid.composer.h,
        "buttons sit below the form, not in a window footer"
    );
    assert!(
        (laid.send.x + laid.send.w - right).abs() < 0.5,
        "Send owns the column's right edge"
    );
}

/// General feedback is the first card and the one chosen when the page
/// opens; "Something isn't working" is last.
#[test]
fn general_feedback_comes_first_and_is_chosen_by_default() {
    assert_eq!(Kind::all(), [Kind::General, Kind::Idea, Kind::Bug]);
    assert_eq!(Kind::default(), Kind::General);
}
