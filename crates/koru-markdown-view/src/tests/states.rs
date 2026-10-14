// SPDX-License-Identifier: MPL-2.0

//! The deterministic states, the ratio, and the edit.

use koru_draw::text::TextEngine;
use koru_i18n::Lang;

use super::CANONICAL;
use crate::{BLOCKS_SHOWN, Content, MarkdownDocument, MarkdownView};

const WIDTH: f32 = 560.0;

fn laid(content: Content) -> (MarkdownView, TextEngine) {
    let mut engine = TextEngine::new();
    let mut view = MarkdownView::new();
    view.set_content(content);
    view.relayout(&mut engine, WIDTH);
    (view, engine)
}

fn document(markdown: &str) -> Content {
    Content::Document(Box::new(
        MarkdownDocument::of(markdown).expect("a document"),
    ))
}

#[test]
fn a_rendered_document_draws_its_text_without_its_markers() {
    let (view, _) = laid(document(CANONICAL));
    let lines = view.lines();
    assert!(
        lines.iter().any(|line| line == "The canonical document"),
        "{lines:?}"
    );
    assert!(
        lines.iter().any(|line| line.contains("\u{2022} a bullet")),
        "a bullet is drawn as one: {lines:?}"
    );
    assert!(
        lines.iter().any(|line| line.contains('\u{2611}')),
        "a ticked box is drawn as one: {lines:?}"
    );
    assert!(
        !lines.iter().any(|line| line.starts_with("# ")),
        "no line still wears its Markdown: {lines:?}"
    );
}

#[test]
fn every_state_says_something() {
    for content in [
        Content::Reading,
        Content::Unreadable,
        Content::Elsewhere { device: None },
        Content::Elsewhere {
            device: Some("The other desk".into()),
        },
        Content::NotMarkdown {
            mime: "application/pdf".into(),
            size_bytes: 4_404_019,
            filename: Some("findings.pdf".into()),
        },
        document(""),
    ] {
        let (view, _) = laid(content.clone());
        assert!(!view.lines().is_empty(), "{content:?} drew nothing at all");
        assert!(view.height() > 0.0, "{content:?} took no room");
    }
}

#[test]
fn a_result_this_device_does_not_hold_names_the_device_that_does() {
    let (view, _) = laid(Content::Elsewhere {
        device: Some("Studio".into()),
    });
    assert!(
        view.lines().iter().any(|line| line.contains("Studio")),
        "{:?}",
        view.lines()
    );
}

#[test]
fn something_that_is_not_markdown_says_what_it_is_and_how_big() {
    let (view, _) = laid(Content::NotMarkdown {
        mime: "application/pdf".into(),
        size_bytes: 4_404_019,
        filename: Some("findings.pdf".into()),
    });
    let lines = view.lines().join("\n");
    assert!(lines.contains("application/pdf"), "{lines}");
    assert!(lines.contains("4.2 MB"), "{lines}");
    assert!(lines.contains("findings.pdf"), "{lines}");
}

/// **The rule the roadmap set for P1.13, here.** A document a thousand times
/// longer lays out the same number of blocks.
#[test]
fn opening_a_long_document_costs_what_is_shown_and_not_what_it_holds() {
    let short = (0..5)
        .map(|index| format!("Paragraph {index}.\n"))
        .collect::<Vec<_>>()
        .join("\n");
    let long = (0..5_000)
        .map(|index| format!("Paragraph {index}.\n"))
        .collect::<Vec<_>>()
        .join("\n");

    let (short_view, _) = laid(document(&short));
    let (long_view, _) = laid(document(&long));

    assert_eq!(short_view.work().blocks, 5);
    assert_eq!(long_view.work().blocks, BLOCKS_SHOWN);
    assert_eq!(long_view.remaining(), 5_000 - BLOCKS_SHOWN);
    assert!(
        long_view.work().layouts <= short_view.work().layouts + BLOCKS_SHOWN,
        "a long document laid out {} layouts",
        long_view.work().layouts
    );
}

#[test]
fn showing_the_rest_lays_out_one_more_page_and_no_more() {
    let long = (0..5_000)
        .map(|index| format!("Paragraph {index}.\n"))
        .collect::<Vec<_>>()
        .join("\n");
    let (mut view, mut engine) = laid(document(&long));
    view.reveal_more();
    view.relayout(&mut engine, WIDTH);
    assert_eq!(view.work().blocks, BLOCKS_SHOWN * 2);
}

#[test]
fn an_edit_opens_on_the_document_s_own_markdown_and_is_what_a_save_writes() {
    let mut engine = TextEngine::new();
    let mut view = MarkdownView::new();
    view.set_content(document(CANONICAL));
    assert!(!view.is_editing());
    assert_eq!(view.edited(), None);

    assert!(view.begin_edit(&mut engine));
    assert!(view.is_editing());
    assert_eq!(view.edited().as_deref(), Some(CANONICAL));
    assert!(!view.changed(), "nothing typed is not a change");

    view.relayout(&mut engine, WIDTH);
    assert!(view.height() > 0.0);

    view.cancel_edit();
    assert!(!view.is_editing());
    assert_eq!(view.edited(), None);
}

#[test]
fn nothing_that_is_not_a_document_can_be_edited() {
    let mut engine = TextEngine::new();
    for content in [
        Content::Reading,
        Content::Unreadable,
        Content::Elsewhere { device: None },
        Content::NotMarkdown {
            mime: "image/png".into(),
            size_bytes: 12,
            filename: None,
        },
    ] {
        let mut view = MarkdownView::new();
        view.set_content(content);
        assert!(!view.is_document());
        assert!(!view.begin_edit(&mut engine));
    }
}

#[test]
fn a_saved_document_replaces_what_is_on_screen() {
    let mut engine = TextEngine::new();
    let mut view = MarkdownView::new();
    view.set_content(document(CANONICAL));
    view.begin_edit(&mut engine);
    let saved = MarkdownDocument::of("# Renamed\n\nOne line.\n").expect("a document");
    view.accept_saved(saved);
    view.relayout(&mut engine, WIDTH);
    assert!(!view.is_editing());
    assert!(view.lines().iter().any(|line| line == "Renamed"));
}

/// **New bytes drop an open edit.** Keeping the composer would let a save
/// write an edit of something that is no longer there.
#[test]
fn new_content_drops_an_open_edit() {
    let mut engine = TextEngine::new();
    let mut view = MarkdownView::new();
    view.set_content(document(CANONICAL));
    view.begin_edit(&mut engine);
    view.set_content(Content::Reading);
    assert!(!view.is_editing());
}

/// A comment is shown — it is part of what a model receives — and it is not
/// shown as though it were the instructions.
#[test]
fn a_comment_is_shown_and_is_not_the_document_s_own_voice() {
    let (view, _) = laid(document(
        "<!-- a note to whoever edits this -->\n\n# Title\n\nBody.\n",
    ));
    let lines = view.lines().join("\n");
    assert!(lines.contains("a note to whoever edits this"), "{lines}");
}

#[test]
fn every_state_says_something_in_all_eight_languages() {
    for lang in Lang::ALL {
        for content in [
            Content::Reading,
            Content::Unreadable,
            Content::Elsewhere { device: None },
            Content::NotMarkdown {
                mime: "application/zip".into(),
                size_bytes: 1_024,
                filename: None,
            },
            document(""),
        ] {
            let mut engine = TextEngine::new();
            let mut view = MarkdownView::new();
            view.set_lang(lang);
            view.set_content(content.clone());
            view.relayout(&mut engine, WIDTH);
            let lines = view.lines();
            assert!(
                lines.iter().any(|line| !line.trim().is_empty()),
                "{content:?} says nothing in {}",
                lang.tag()
            );
        }
    }
}

/// **Rule 7.** A document's text and a file's name are content. They go on
/// the screen and nowhere else — not into a `Debug`, which is what reaches a
/// log.
#[test]
fn no_document_text_and_no_filename_reaches_a_debug_line() {
    let secret = "SECRET-PARAGRAPH-nobody-logs-this";
    let document = MarkdownDocument::of(&format!("# Mine\n\n{secret}\n")).expect("a document");
    let printed = format!("{document:?}");
    assert!(!printed.contains(secret), "{printed}");
    assert!(
        printed.contains("bytes") && printed.contains("blocks"),
        "{printed}"
    );

    let content = Content::Document(Box::new(document));
    assert!(!format!("{content:?}").contains(secret));

    let named = Content::NotMarkdown {
        mime: "application/pdf".into(),
        size_bytes: 12,
        filename: Some("SECRET-FILENAME.pdf".into()),
    };
    let printed = format!("{named:?}");
    assert!(!printed.contains("SECRET-FILENAME"), "{printed}");
    assert!(printed.contains("application/pdf"), "{printed}");

    let elsewhere = Content::Elsewhere {
        device: Some("SECRET-DEVICE".into()),
    };
    assert!(!format!("{elsewhere:?}").contains("SECRET-DEVICE"));
}
