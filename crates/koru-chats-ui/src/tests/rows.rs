// SPDX-License-Identifier: MPL-2.0

//! What the list shows, in what order, and where.

use super::*;
use crate::text::who_answered;
use crate::{Load, Notice};

#[test]
fn chats_are_listed_newest_first_whatever_order_they_arrive_in() {
    let surface = three();
    assert_eq!(titles(&surface), ["Newest", "Middle", "Oldest"]);
}

#[test]
fn new_chat_heads_the_list_and_every_chat_is_a_row() {
    let mut surface = three();
    assert_eq!(
        rows_of(&mut surface),
        [Row::New, Row::Chat(0), Row::Chat(1), Row::Chat(2)]
    );
}

#[test]
fn a_cold_surface_says_it_is_loading_rather_than_empty() {
    let mut surface = ChatsSurface::without_reader();
    assert_eq!(surface.load(), Load::Cold);
    assert_eq!(
        rows_of(&mut surface),
        [Row::New, Row::Notice(Notice::Loading)]
    );
}

#[test]
fn an_empty_answer_is_the_empty_notice() {
    let mut surface = showing(Vec::new());
    assert_eq!(
        rows_of(&mut surface),
        [Row::New, Row::Notice(Notice::Empty)]
    );
}

/// **A service that stops answering is not an empty list.** What was on
/// screen stays, under a sentence saying why it may be old.
#[test]
fn an_unavailable_service_keeps_the_rows_and_says_so() {
    let mut surface = three();
    surface.take_listing(Err(crate::gateway::unreachable()));
    assert_eq!(surface.load(), Load::Unavailable);
    assert_eq!(
        rows_of(&mut surface),
        [
            Row::New,
            Row::Notice(Notice::Unavailable),
            Row::Chat(0),
            Row::Chat(1),
            Row::Chat(2)
        ]
    );
}

#[test]
fn who_answered_speaks_the_chips_vocabulary() {
    let lang = koru_i18n::Lang::En;
    let agents = [opencode()];
    // An agent by its own name, with the model only when one was picked.
    assert_eq!(
        who_answered(lang, &model("agent-opencode", "default"), &agents),
        "OpenCode"
    );
    assert_eq!(
        who_answered(lang, &model("agent-opencode", "deepseek/v4-pro"), &agents),
        "OpenCode · deepseek/v4-pro"
    );
    // An agent this device no longer lists is still a local agent, never a
    // token.
    assert_eq!(
        who_answered(lang, &model("agent-codex", "default"), &agents),
        "Local agent"
    );
    // OpenAI and Ollama by the model a person chose; Koru by its label.
    assert_eq!(
        who_answered(lang, &model("openai", "gpt-6-astra"), &agents),
        "gpt-6-astra"
    );
    assert_eq!(
        who_answered(lang, &model("ollama", "qwen3:8b"), &agents),
        "qwen3:8b"
    );
    assert_eq!(
        who_answered(lang, &model("koru", "Qwen/Qwen3-8B"), &agents),
        koru_ai_core::AiProviderId::KORU_HOSTED_MODEL_LABEL
    );
}

#[test]
fn a_row_carries_who_answered_and_when() {
    let surface = three();
    let line = crate::text::meta_line(
        koru_i18n::Lang::En,
        &surface.chats()[1],
        surface.agents(),
        NOW,
    );
    assert!(line.starts_with("gpt-6-astra · "), "{line}");
    assert!(line.contains("minute"), "{line}");
}

/// **One clipped rectangle.** A row scrolled above the band is neither
/// returned nor hit, and a press below the band's floor is nobody's.
#[test]
fn the_band_is_the_cull_and_the_hit_test() {
    let many: Vec<_> = (1..=60)
        .map(|n| chat(n, &format!("Chat {n}"), i64::from(n) * 1_000))
        .collect();
    let mut surface = showing(many);
    let area = area();
    surface.scroll_by(area, TOP, -400.0);
    let visible = surface.rows(area, TOP);
    let band = surface.band(area, TOP);
    assert!(!visible.is_empty());
    for (_, rect) in &visible {
        assert!(rect.y + rect.h > band.y && rect.y < band.y + band.h);
    }
    // Above the band — where the heading is — nothing answers.
    assert_eq!(surface.row_at(area, TOP, 50.0, TOP - 5.0), None);
    // Below the floor, nothing answers either.
    assert_eq!(surface.row_at(area, TOP, 50.0, area.h - 2.0), None);
    // Inside it, the row that is drawn there answers.
    let (row, rect) = visible[visible.len() / 2];
    let hit = surface.row_at(area, TOP, 50.0, rect.y + rect.h * 0.5);
    assert_eq!(hit.map(|(row, _)| row), Some(row));
}

#[test]
fn every_label_starts_on_one_line() {
    let mut surface = three();
    let rows = surface.rows(tall(), TOP);
    let xs: Vec<f32> = rows
        .iter()
        .filter(|(row, _)| row.pressable())
        .map(|(row, rect)| surface.row_parts(*row, *rect).title.x)
        .collect();
    assert!(xs.windows(2).all(|pair| pair[0] == pair[1]), "{xs:?}");
}

#[test]
fn the_accessibility_rects_are_the_drawn_rects_unculled() {
    let many: Vec<_> = (1..=60)
        .map(|n| chat(n, &format!("Chat {n}"), i64::from(n) * 1_000))
        .collect();
    let mut surface = showing(many);
    let area = area();
    let published = surface.chat_rects(area, TOP);
    // Every chat is published, including those below the fold.
    assert_eq!(published.len(), 60);
    // And each one that is drawn is published where it is drawn.
    for (row, rect) in surface.rows(area, TOP) {
        if let Row::Chat(index) = row {
            assert_eq!(published[index], (surface.chats()[index].id, rect));
        }
    }
}
