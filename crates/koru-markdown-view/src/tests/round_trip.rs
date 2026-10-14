// SPDX-License-Identifier: MPL-2.0

//! **The round trip**, in the two halves that matter.
//!
//! *Exact* for the shapes Koru has a node for, and *a fixed point* for
//! everything else — which together are the promise a save makes: what you
//! read is what is written, and saving an untouched document a second time
//! changes nothing.

use super::CANONICAL;
use crate::MarkdownDocument;

/// Every shape Koru supports survives the document unchanged.
#[test]
fn the_canonical_document_exports_to_exactly_what_it_was() {
    let document = MarkdownDocument::of(CANONICAL).expect("a document");
    assert_eq!(document.markdown(), CANONICAL);
}

/// And each shape is still a block of its own, rather than a wall of text.
#[test]
fn the_canonical_document_keeps_its_shapes_as_blocks() {
    use koru_import_markdown::{Block, TextKind};
    let document = MarkdownDocument::of(CANONICAL).expect("a document");
    let blocks = document.blocks();
    assert!(
        blocks
            .iter()
            .any(|block| matches!(block, Block::Text(text) if text.heading.is_some())),
        "a heading"
    );
    assert!(
        blocks
            .iter()
            .any(|block| matches!(block, Block::Text(text) if text.list.is_some())),
        "a list item"
    );
    assert!(
        blocks
            .iter()
            .any(|block| matches!(block, Block::Text(text) if text.task == Some(true))),
        "a ticked box"
    );
    assert!(
        blocks.iter().any(
            |block| matches!(block, Block::Text(text) if matches!(text.kind, TextKind::Quote))
        ),
        "a quotation"
    );
    assert!(
        blocks.iter().any(
            |block| matches!(block, Block::Text(text) if matches!(text.kind, TextKind::Code { .. }))
        ),
        "a code block"
    );
    assert!(
        blocks.iter().any(|block| matches!(block, Block::Table(_))),
        "a table"
    );
    assert!(
        blocks.iter().any(|block| matches!(block, Block::Rule)),
        "a rule"
    );
    assert!(
        blocks.iter().any(
            |block| matches!(block, Block::Text(text) if text.runs.iter().any(|run| run.marks.bold))
        ),
        "a bold run"
    );
}

/// **Whatever the text, one pass settles it.** Markdown Koru would spell
/// differently is spelled Koru's way once and then never again — which is
/// why a save of an untouched document is a no-op rather than a slow drift
/// through the file.
#[test]
fn any_markdown_is_a_fixed_point_after_one_pass() {
    for source in [
        CANONICAL,
        "no heading at all, just a line",
        "# Title\n\nsomething with (brackets) and [square] ones and a #hash\n",
        "<!--\na comment\nover two lines\n-->\n\n# After it\n\nBody.\n",
        "* one\n* two\n\n  nested paragraph\n",
        "Text with a [link](https://example.test/a) in it.\n",
    ] {
        let once = MarkdownDocument::of(source).expect("a document");
        let twice = MarkdownDocument::of(once.markdown()).expect("a document");
        assert_eq!(
            once.markdown(),
            twice.markdown(),
            "not a fixed point: {source:?}"
        );
    }
}

/// The preamble Koru ships is real Markdown a person edits, so it is the
/// fixture worth proving the fixed point on.
#[test]
fn the_shipped_preamble_is_a_fixed_point_too() {
    let source = include_str!("../../../koru-delegate-preamble/delegated-agent.md");
    let once = MarkdownDocument::of(source).expect("a document");
    let twice = MarkdownDocument::of(once.markdown()).expect("a document");
    assert_eq!(once.markdown(), twice.markdown());
}

/// An HTML comment is carried through as text — it is part of what a model
/// receives — and is recognisable, so a surface can draw it quietly.
#[test]
fn an_html_comment_is_carried_through_and_recognisable() {
    let document =
        MarkdownDocument::of("<!-- a note to whoever edits this -->\n\n# Title\n\nBody.\n")
            .expect("a document");
    assert!(
        document.markdown().contains("<!--"),
        "{}",
        document.markdown()
    );
    let comments = document
        .blocks()
        .iter()
        .filter(|block| crate::is_comment(block))
        .count();
    assert_eq!(comments, 1);
}

/// An empty file is a document with nothing in it, not a failure.
#[test]
fn an_empty_file_is_an_empty_document() {
    let document = MarkdownDocument::of("").expect("a document");
    assert!(document.is_empty());
}

// ---------------------------------------------------------------------------
// **The measurement, construct by construct** (ADR-378).
//
// Editing a result as a document means its bytes make a round trip —
// Markdown in, Koru document, Markdown out — and whatever the node model
// cannot hold changes on the way. These are that measurement, written down
// so that a year from now the answer to *what does saving do to a table* is
// a test rather than an argument. Each pair is exactly what this build does.
// ---------------------------------------------------------------------------

/// What one pass makes of a piece of Markdown.
fn through(source: &str) -> String {
    MarkdownDocument::of(source)
        .expect("a document")
        .markdown()
        .to_owned()
}

/// **The one that mattered.** The maintainers' own result is a pipe table with a
/// bold run and a link in its cells, and it comes back byte for byte — cell
/// text, alignment row and inline marks included. A save that turned it into
/// six paragraphs of pipes would have destroyed the thing this feature is
/// for.
///
/// It survives because a table is not decomposed at all: `Kind::Table` holds
/// GFM source, which the exporter writes back out (its own module doc says
/// so).
#[test]
fn a_pipe_table_survives_byte_for_byte() {
    for table in [
        "| Name | Detail |\n| --- | --- |\n| **a** | [link](https://example.test/x) |\n| b | 2 |\n",
        "| L | C | R |\n|:--- |:---:| ---:|\n| 1 | 2 | 3 |\n",
        "| a | b |\n| --- | --- |\n| **x** | `y` |\n",
        "| a | b |\n| --- | --- |\n| 1 | 2 |\n\nAfter the table.\n",
    ] {
        assert_eq!(through(table), table, "a table must not be rewritten");
    }
    // And it is one block, not a wall of pipes.
    let document = MarkdownDocument::of(
        "| Name | Detail |\n| --- | --- |\n| **a** | [link](https://example.test/x) |\n",
    )
    .expect("a document");
    assert_eq!(document.blocks().len(), 1);
    assert!(matches!(
        document.blocks()[0],
        koru_import_markdown::Block::Table(_)
    ));
    assert!(document.is_faithful());
}

/// **What comes back exactly as it went in.** Everything Koru has a node for,
/// each written the way the exporter writes it — which is what a result an
/// agent produced looks like, because an agent's Markdown is ordinary
/// Markdown.
#[test]
fn what_survives_byte_for_byte() {
    for source in [
        // Headings, at every level.
        "# H1\n\n## H2\n\n### H3\n\n#### H4\n\n##### H5\n\n###### H6\n",
        // Numbered sections, which are headings and not a list.
        "## 1. First\n\nBody.\n\n## 2. Second\n\nBody.\n",
        // Marks.
        "A paragraph with **bold**, *italic* and ***both***.\n",
        "This is ~~gone~~ now.\n",
        "Use `cargo test -p x` for that.\n",
        // Links: inline, alone on a line, and a bare URL that is only text.
        "See the [manual](https://example.test/a) for more.\n",
        "[the manual](https://example.test/a)\n",
        "https://example.test/a\n",
        // Lists, nested and mixed.
        "- one\n  - one a\n    - one a i\n- two\n",
        "1. one\n2. two\n3. three\n",
        "- one\n  1. a\n  2. b\n",
        "- [ ] todo\n- [x] done\n",
        // Blocks.
        "> a quotation\n",
        "```rust\nfn main() { let x = 1; }\n```\n",
        "```\nplain\n```\n",
        "before\n\n---\n\nafter\n",
        // Punctuation the author escaped stays escaped.
        "5 \\* 3, a \\_literal\\_ underscore, and \\# not a heading\n",
        // Text that is not ASCII.
        "café — naïve · 日本語 · 🎉\n",
    ] {
        assert_eq!(through(source), source, "{source:?} was rewritten");
        assert!(
            MarkdownDocument::of(source)
                .expect("a document")
                .is_faithful(),
            "{source:?} says it was rewritten"
        );
    }
}

/// **What is rewritten, and into exactly what.**
///
/// Every one of these is Markdown Koru has no node for, or spells its own
/// way. The left column is what somebody wrote; the right column is what a
/// save writes back. None of it is new — the composer has been loaded from
/// the exporter's Markdown since this surface existed, so this is what a save
/// has always done — but it is the reason a version that does not survive its
/// own round trip is *told about* before it is edited (ADR-378).
#[test]
fn what_is_rewritten_and_into_what() {
    // Meaning kept, spelling Koru's own.
    let respelled = [
        // Setext becomes ATX.
        ("Title\n=====\n\nBody.\n", "# Title\n\nBody.\n"),
        // Indented code becomes fenced code.
        (
            "    indented code\n    second line\n",
            "```\nindented code\nsecond line\n```\n",
        ),
        // An autolink becomes an ordinary link.
        (
            "<https://example.test/a>\n",
            "[https://example.test/a](https://example.test/a)\n",
        ),
        // Punctuation that would be a mark is escaped so it stays literal.
        (
            "a_b_c and snake_case_name\n",
            "a\\_b\\_c and snake\\_case\\_name\n",
        ),
        // Blank lines collapse and trailing spaces go.
        ("one\n\n\n\ntwo\n", "one\n\ntwo\n"),
        ("some text   \n", "some text\n"),
        // A soft wrap is a space, because a paragraph is one run of text.
        ("line one\nline two\n", "line one line two\n"),
    ];
    // Meaning changed. Each is a thing the node model cannot hold.
    let rewritten = [
        // An ordered list is renumbered from 1: a list item knows it is
        // numbered, not which number it was given.
        ("3. three\n4. four\n", "1. three\n2. four\n"),
        // A hard line break is not a node, so the line joins.
        ("line one  \nline two\n", "line one line two\n"),
        ("line one\\\nline two\n", "line one line two\n"),
        // A blockquote of two paragraphs becomes two blockquotes.
        ("> one\n>\n> two\n", "> one\n\n> two\n"),
        // Frontmatter becomes a visible YAML code block.
        (
            "---\ntitle: x\n---\n\nBody.\n",
            "```yaml\ntitle: x\n```\n\nBody.\n",
        ),
        // An HTML block is one paragraph of text, its own line breaks gone.
        (
            "<div class=\"x\">\n  <p>hi</p>\n</div>\n",
            "<div class=\"x\">  <p>hi</p></div>\n",
        ),
        // A comment butted against the paragraph under it merges with it.
        // (A comment above a *heading* does not — `an_html_comment_is_
        // carried_through_and_recognisable` is that case.)
        ("<!-- a note -->\n\nBody.\n", "<!-- a note -->Body.\n"),
        // An image whose bytes are not here becomes a link to them.
        (
            "![alt text](https://example.test/pic.png)\n",
            "[alt text](https://example.test/pic.png)\n",
        ),
        // A footnote is not a node: the marker is escaped into literal text
        // and the definition becomes a paragraph.
        (
            "Text with a footnote[^1].\n\n[^1]: The note.\n",
            "Text with a footnote\\[^1\\].\n\n\\[^1\\]: The note.\n",
        ),
        // Neither is a hand-written `[1]` reference list, and its lines join.
        (
            "As shown in [1].\n\n[1] Somebody, 2020.\n[2] Someone else, 2021.\n",
            "As shown in \\[1\\].\n\n\\[1\\] Somebody, 2020. \\[2\\] Someone else, 2021.\n",
        ),
        // A reference link is resolved inline, and its definition line is
        // left behind as a paragraph rather than dropped.
        (
            "See [the manual][m].\n\n[m]: https://example.test/a\n",
            "See [the manual](https://example.test/a).\n\n[\\[m\\] https://example.test/a](https://example.test/a)\n",
        ),
    ];
    for (source, expected) in respelled.iter().chain(rewritten.iter()) {
        assert_eq!(&through(source), expected, "for {source:?}");
        assert!(
            !MarkdownDocument::of(source)
                .expect("a document")
                .is_faithful(),
            "{source:?} was rewritten and did not say so"
        );
        // **And once only.** Whatever it became, it stays that from now on.
        assert_eq!(
            &through(expected),
            expected,
            "not a fixed point: {source:?}"
        );
    }
}

/// **A document says whether it is the bytes it came from**, which is the
/// whole of what the tab needs in order to warn before the first keystroke.
#[test]
fn a_document_knows_whether_it_would_rewrite_its_own_source() {
    assert!(
        MarkdownDocument::of(CANONICAL)
            .expect("a document")
            .is_faithful()
    );
    assert!(
        !MarkdownDocument::of("3. three\n4. four\n")
            .expect("a document")
            .is_faithful()
    );
    // A document that was never read from bytes has no source to differ
    // from: it *is* the source. This is the way back out of an edit.
    let built = MarkdownDocument::of("3. three\n4. four\n").expect("a document");
    let doc = koru_core::schema::WorkspaceDoc::from_snapshot(built.snapshot()).expect("a doc");
    let exported = MarkdownDocument::of_document(&doc).expect("a document");
    assert!(exported.is_faithful());
    assert_eq!(exported.markdown(), built.markdown());
}
