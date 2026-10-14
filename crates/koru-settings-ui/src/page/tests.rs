// SPDX-License-Identifier: MPL-2.0

//! The builder's choices: which control a setting gets, and how cards close.

use super::*;
use koru_settings::DensityChoice;

#[test]
fn search_finds_translated_page_names_and_settings() {
    for lang in Lang::ALL {
        for page in PageId::ALL {
            assert!(page.matches(lang, page.label(lang)));
        }
        assert!(PageId::Appearance.matches(lang, koru_i18n::t(lang, Key::SettingsDensity)));
        assert!(PageId::Editor.matches(lang, koru_i18n::t(lang, Key::SpellingPersonalDictionary)));
        assert!(PageId::Terminal.matches(lang, koru_i18n::t(lang, Key::SettingsZoom)));
    }
    assert!(PageId::Editor.matches(Lang::Es, "  DICCIONARIO  "));
    assert!(!PageId::Reminders.matches(Lang::Es, "diccionario"));
}

fn builder(engine: &mut TextEngine) -> Builder<'_> {
    Builder::new(engine, 100.0, 600.0, 40.0)
}

fn density(label: &str) -> (Action, String, bool) {
    (
        Action::Density(DensityChoice::Normal),
        label.to_owned(),
        false,
    )
}

#[test]
fn a_few_short_values_are_a_segmented_track_and_many_are_a_dropdown() {
    let mut engine = TextEngine::new();
    let mut b = builder(&mut engine);
    b.card(Some("Card"));
    b.row_choice("Few", None, vec![density("A"), density("B")]);
    b.row_choice(
        "Many",
        None,
        vec![density("A"), density("B"), density("C"), density("D")],
    );
    b.row_choice(
        "Wide",
        None,
        vec![
            density("An exceptionally long first value"),
            density("And an even longer second value than that"),
        ],
    );
    let laid = b.finish();
    let Block::Card(card) = &laid.blocks[0] else {
        panic!("a card")
    };
    assert!(matches!(card.rows[0].widget, Widget::Segmented { .. }));
    assert!(matches!(card.rows[1].widget, Widget::Dropdown { .. }));
    assert!(
        matches!(card.rows[2].widget, Widget::Dropdown { .. }),
        "a track wider than the row's share becomes a dropdown"
    );
}

#[test]
fn rows_stack_inside_their_card_and_the_next_card_starts_below_it() {
    let mut engine = TextEngine::new();
    let mut b = builder(&mut engine);
    b.card(Some("First"));
    b.row_text("One", Some("A sentence under it."));
    b.row_text("Two", None);
    b.card(Some("Second"));
    b.row_text("Three", None);
    let laid = b.finish();
    let cards: Vec<&CardLaid> = laid
        .blocks
        .iter()
        .filter_map(|block| match block {
            Block::Card(card) => Some(card),
            _ => None,
        })
        .collect();
    assert_eq!(cards.len(), 2);
    let first = cards[0];
    assert_eq!(
        first.rows[0].rect.y,
        first.rect.y + Card::new(Some("First")).header_h()
    );
    assert_eq!(
        first.rows[1].rect.y,
        first.rows[0].rect.y + first.rows[0].rect.h
    );
    let last = first.rows.last().unwrap();
    assert_eq!(first.rect.y + first.rect.h, last.rect.y + last.rect.h);
    assert_eq!(cards[1].rect.y, first.rect.y + first.rect.h + Card::GAP);
    assert!(laid.bottom > cards[1].rect.y + cards[1].rect.h);
}

#[test]
fn an_empty_card_is_not_drawn_and_gives_its_room_back() {
    let mut engine = TextEngine::new();
    let mut b = builder(&mut engine);
    b.card(Some("Empty"));
    b.card(Some("Full"));
    b.row_text("Row", None);
    let laid = b.finish();
    let cards: Vec<&CardLaid> = laid
        .blocks
        .iter()
        .filter_map(|block| match block {
            Block::Card(card) => Some(card),
            _ => None,
        })
        .collect();
    assert_eq!(cards.len(), 1);
    assert_eq!(cards[0].title.as_deref(), Some("Full"));
    assert_eq!(
        cards[0].rect.y, 40.0,
        "it starts where the empty one would have"
    );
}

#[test]
fn every_control_sits_in_its_row_against_the_right_edge() {
    let mut engine = TextEngine::new();
    let mut b = builder(&mut engine);
    b.card(None);
    b.row_toggle("Switch", None, true, Action::StartOnLogin(false));
    let laid = b.finish();
    let Block::Card(card) = &laid.blocks[0] else {
        panic!("a card")
    };
    let row = &card.rows[0];
    let Widget::Toggle { rect, hit, .. } = &row.widget else {
        panic!("a toggle")
    };
    assert!(row.rect.contains(rect.x, rect.y));
    assert_eq!(rect.x + rect.w, row.rect.x + row.rect.w - CardRow::PAD_X);
    assert_eq!(laid.hits[*hit].rect, *rect);
}
