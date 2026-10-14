// SPDX-License-Identifier: MPL-2.0

//! **The finding, asserted.** A result the maintainers opened came out as a text
//! dump — a bullet list drawn as paragraphs each beginning with a literal
//! `•`, headings in the same ink and size as body text, a link underlined
//! blue mid-paragraph. Those are three facts about the *model* the page is
//! drawn from, so that is where they are checked: a bullet is a bullet
//! because the row is one, not because a renderer put a dot in front of the
//! words.

use super::*;
use koru_artifacts_ui::MarkdownDocument;
use koru_core::{Heading, ListStyle};

const FINDINGS: &str = "# Quarterly findings\n\n\
Revenue held, and the [renewal note](https://example.test/renewals) explains why.\n\n\
- the second region\n\
- the renewal in March\n";

fn page() -> ResultDocument {
    let document = MarkdownDocument::of(FINDINGS).expect("a document");
    ResultDocument::of(document.snapshot(), false).expect("a page")
}

/// Every row of the page, as the model has it: what it is, its heading
/// level, its list style and its text.
fn rows(page: &ResultDocument) -> Vec<(Option<Heading>, Option<ListStyle>, String)> {
    page.leaf()
        .body()
        .rows
        .iter()
        .map(|row| (row.heading, row.list_style, row.text.clone()))
        .collect()
}

/// **A bullet is a bullet.** Not a paragraph that starts with `\u{2022}`.
#[test]
fn a_bullet_list_is_a_list_and_not_paragraphs_wearing_a_dot() {
    let page = page();
    let rows = rows(&page);
    let bullets: Vec<_> = rows.iter().filter(|(_, list, _)| list.is_some()).collect();
    assert_eq!(bullets.len(), 2, "{rows:?}");
    for (_, list, text) in bullets {
        assert_eq!(*list, Some(ListStyle::Bullet), "{rows:?}");
        assert!(
            !text.contains('\u{2022}'),
            "the marker is drawn, never typed: {text:?}"
        );
    }
    assert!(
        rows.iter()
            .any(|(_, list, text)| list.is_some() && text == "the second region"),
        "{rows:?}"
    );
}

/// **A heading is a heading**, at the level it was written at, rather than a
/// paragraph that happens to be first.
#[test]
fn a_heading_keeps_its_level() {
    let rows = rows(&page());
    assert!(
        rows.iter()
            .any(|(heading, _, text)| *heading == Some(Heading::H1)
                && text == "Quarterly findings"),
        "{rows:?}"
    );
    assert!(
        rows.iter()
            .filter(|(heading, _, _)| heading.is_some())
            .count()
            == 1,
        "and only the one that was written as one: {rows:?}"
    );
}

/// **A link is a link** — Koru's own link pill, carrying the target and the
/// words the author wrote, which is what makes it something to follow rather
/// than blue text in the middle of a sentence.
#[test]
fn a_link_survives_as_a_link_pill() {
    let page = page();
    let pills: Vec<_> = page
        .leaf()
        .body()
        .rows
        .iter()
        .flat_map(|row| row.pills.values())
        .filter(|pill| pill.kind == koru_draw::pill::PillKind::Link)
        .collect();
    assert_eq!(pills.len(), 1, "{pills:?}");
    assert_eq!(pills[0].label, "renewal note");
    assert_eq!(
        pills[0].url.as_deref(),
        Some("https://example.test/renewals")
    );
}

/// **The page carries no title row.** The result's name is the tab's, and
/// `document_from_text` adds none — so a title row here would be an empty
/// placeholder over somebody's first heading.
#[test]
fn the_page_has_no_title_row_of_its_own() {
    let page = page();
    assert!(!page.leaf().body().is_title_row(0));
    assert_eq!(
        page.leaf().body().rows.first().map(|row| row.heading),
        Some(Some(Heading::H1)),
        "the document's own first line is the first row"
    );
}

/// **Read, never written.** The page refuses everything outside the reading
/// allowlist, and takes no text anywhere.
#[test]
fn the_page_is_read_only_and_still_selects() {
    use winit::keyboard::{Key, ModifiersState, NamedKey};
    let mut page = page();
    page.set_viewport(900.0, 600.0);
    assert!(page.leaf().view().read_only);

    let before = rows(&page);
    let typed = page.key(
        &Key::Character("x".into()),
        Some("x"),
        ModifiersState::empty(),
    );
    assert!(!typed, "a printable character is refused");
    assert!(
        page.key(
            &Key::Named(NamedKey::ArrowDown),
            None,
            ModifiersState::SHIFT
        ),
        "Shift and an arrow extend a selection"
    );
    assert_eq!(rows(&page), before, "nothing about the page changed");
}

/// **The chrome and the page share one column**, so a result's name sits
/// over its prose rather than beside it.
#[test]
fn the_text_column_is_the_document_views_own() {
    let (x, w) = ResultDocument::text_column(1_200.0);
    let inset = koru_draw::geometry::text_x(0u32, false);
    assert_eq!(x, MODE.content_x(1_200.0) + inset);
    assert_eq!(w, MODE.effective_w(1_200.0) - inset * 2.0);
    // Centred: the same air on both sides.
    assert!((x + w + x - 1_200.0).abs() < 0.5, "{x} + {w}");
}

/// Bytes that are not a workspace holding a document produce no page, so
/// the tab keeps saying what it can rather than showing an empty one.
#[test]
fn bytes_that_are_not_a_document_open_nothing() {
    assert!(ResultDocument::of(b"not a snapshot", false).is_none());
}

/// **The snapshot a `MarkdownDocument` carries always opens.** This is the
/// invariant that makes the tab's rule safe to state one way: the tab hands
/// the document over and stops drawing it, so a snapshot this view refused
/// would be a blank column under a result's name. It cannot happen, because
/// the snapshot is the one `koru-markdown-view` itself imported and exported
/// — and every shape a Markdown file has goes through the same builder.
#[test]
fn every_shape_a_result_can_have_opens_as_a_page() {
    for fixture in [
        "# One\n\nTwo\n",
        "- a\n- b\n\n1. one\n2. two\n",
        "> quoted\n\n```rust\nfn main() {}\n```\n",
        "| a | b |\n| --- | --- |\n| 1 | 2 |\n\n---\n\n![alt](./missing.png)\n",
        "- [x] done\n- [ ] not\n",
        "---\ntags: [one]\n---\nBody\n",
        "just one line",
    ] {
        let document = MarkdownDocument::of(fixture).expect("a document");
        assert!(
            ResultDocument::of(document.snapshot(), false).is_some(),
            "{fixture:?} made no page"
        );
    }
}

// ---------------------------------------------------------------------------
// **Editing the same page** (ADR-378). The page a person reads is the page a
// person types into, with one flag different — and what leaves it is Markdown
// through the exporter, never a second dialect.
// ---------------------------------------------------------------------------

/// An editable page over the same bytes.
fn editable() -> ResultDocument {
    let document = MarkdownDocument::of(FINDINGS).expect("a document");
    ResultDocument::of(document.snapshot(), true).expect("a page")
}

/// **Edit means this page, open for typing.** Same rows, same bullets, same
/// link pill — the read-only flag is the whole of the difference.
#[test]
fn an_editable_page_is_the_same_page_that_takes_text() {
    use winit::keyboard::{Key, ModifiersState};
    let mut open = editable();
    open.set_viewport(900.0, 600.0);
    assert!(open.is_editable());
    assert!(!open.leaf().view().read_only);
    assert_eq!(rows(&open), rows(&page()), "the same document either way");

    assert!(!open.is_edited(), "nothing has been typed yet");
    assert!(
        open.key(
            &Key::Character("x".into()),
            Some("x"),
            ModifiersState::empty()
        ),
        "a printable character is taken"
    );
    assert!(open.is_edited(), "and the page says the document moved");
    assert_ne!(rows(&open), rows(&page()), "the text changed");
}

/// **A page built for reading still refuses.** The two are built from the
/// same bytes by the same function, so the read-only one has to keep saying
/// no — nothing about editing loosened it.
#[test]
fn a_reading_page_still_takes_no_text() {
    use winit::keyboard::{Key, ModifiersState};
    let mut reading = page();
    reading.set_viewport(900.0, 600.0);
    assert!(!reading.is_editable());
    assert!(
        !reading.key(
            &Key::Character("x".into()),
            Some("x"),
            ModifiersState::empty()
        ),
        "a printable character is refused"
    );
    assert!(!reading.is_edited());
}

/// **The way back out is the exporter**, so what a save writes is what the
/// page was drawn from — down to the escaping — until somebody types.
#[test]
fn an_untouched_page_exports_the_markdown_it_was_built_from() {
    let built = MarkdownDocument::of(FINDINGS).expect("a document");
    let open = ResultDocument::of(built.snapshot(), true).expect("a page");
    assert_eq!(open.markdown().as_deref(), Some(built.markdown()));
    // And a read-only page answers the same, because it is the same document.
    assert_eq!(page().markdown().as_deref(), Some(built.markdown()));
}

/// **What is typed is what is exported**, through the shapes that matter:
/// the table survives, the bullet stays a bullet, and the new words are
/// there.
#[test]
fn what_is_typed_reaches_the_markdown_a_save_would_write() {
    use winit::keyboard::{Key, ModifiersState};
    const WITH_A_TABLE: &str = "# Findings\n\n\
| Region | Renewal |\n\
| --- | --- |\n\
| **north** | March |\n\n\
- the second region\n";
    let built = MarkdownDocument::of(WITH_A_TABLE).expect("a document");
    let mut page = ResultDocument::of(built.snapshot(), true).expect("a page");
    page.set_viewport(900.0, 600.0);
    for character in ["N", "e", "w", "."] {
        page.key(
            &Key::Character(character.into()),
            Some(character),
            ModifiersState::empty(),
        );
    }
    let markdown = page.markdown().expect("markdown");
    assert!(markdown.contains("New."), "{markdown}");
    assert!(
        markdown.contains("| **north** | March |"),
        "the table is untouched: {markdown}"
    );
    assert!(markdown.contains("- the second region"), "{markdown}");
}

/// **The workspace is still nobody's.** Editing did not give this page a
/// session, and it did not give two pages built from the same result one
/// document between them: typing into one leaves the other exactly as it
/// was. That is the structural guarantee ADR-372 rests on, asserted now that
/// typing is allowed.
#[test]
fn an_editable_page_reaches_nothing_and_no_other_page() {
    use winit::keyboard::{Key, ModifiersState};
    let mut typed = editable();
    let untouched = editable();
    typed.set_viewport(900.0, 600.0);
    assert!(typed.workspace.session.is_none());
    assert!(untouched.workspace.session.is_none());

    typed.key(
        &Key::Character("x".into()),
        Some("x"),
        ModifiersState::empty(),
    );
    assert!(typed.is_edited());
    assert!(!untouched.is_edited());
    assert_eq!(
        untouched.markdown().as_deref(),
        Some(
            MarkdownDocument::of(FINDINGS)
                .expect("a document")
                .markdown()
        ),
        "a second page over the same result never saw the typing"
    );
}
