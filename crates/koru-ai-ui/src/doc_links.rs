// SPDX-License-Identifier: MPL-2.0

//! A document chip, once it is a message: `[[koru://doc/<id>]]` drawn as the
//! document's **title**, as a link.
//!
//! The composer sends a chip in the Markdown exporter's own spelling
//! (`koru_export_markdown::message_text`), so the agent, the importer and this
//! renderer read one token. What a person reads in the conversation is the
//! title, in the link color, and a click on it opens the document — never the
//! raw token.
//!
//! **Titles are looked up, never fetched here.** They come from what the
//! surface already knows: the chip's own caption when this window sent it, the
//! chat's pinned documents, and answers the reader thread brings back for any
//! id still unknown (`Command::DocumentTitles`). Until one arrives the link
//! reads as the fallback word, and the layout is rebuilt when the title lands,
//! because the displayed text is part of what the layout cache compares.

use std::collections::HashMap;

use koru_ai_core::DocumentId;
use koru_markdown::LinkSpan;

/// Every document a message's chips name, in order, each once.
pub(crate) fn documents(markdown: &str) -> Vec<DocumentId> {
    let mut out: Vec<DocumentId> = Vec::new();
    for found in koru_link::wrapped(markdown) {
        if let koru_link::KoruLink::Document { document, .. } = found.link {
            let id = DocumentId::from_uuid(document);
            if !out.contains(&id) {
                out.push(id);
            }
        }
    }
    out
}

/// `markdown` with every document token replaced by its title, and where
/// each title now sits. The link's URL is the token's own `koru://doc/…`, so
/// a heading chip still opens at its heading.
pub(crate) fn titled(
    markdown: &str,
    titles: &HashMap<DocumentId, String>,
    fallback: &str,
) -> (String, Vec<LinkSpan>) {
    let found = koru_link::wrapped(markdown);
    if found.is_empty() {
        return (markdown.to_owned(), Vec::new());
    }
    let mut text = String::with_capacity(markdown.len());
    let mut links = Vec::new();
    let mut at = 0;
    for token in found {
        let koru_link::KoruLink::Document { document, node } = token.link else {
            continue;
        };
        text.push_str(&markdown[at..token.range.start]);
        let title = titles
            .get(&DocumentId::from_uuid(document))
            .map(String::as_str)
            .filter(|title| !title.trim().is_empty())
            .unwrap_or(fallback);
        let start = text.len();
        text.push_str(title);
        links.push(LinkSpan {
            range: start..text.len(),
            url: koru_link::document_link(document, node),
        });
        at = token.range.end;
    }
    text.push_str(&markdown[at..]);
    (text, links)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_chip_reads_as_its_title_and_links_to_its_document() {
        let document = DocumentId::new();
        let markdown = format!("Summarise [[koru://doc/{document}]] please");
        let titles = HashMap::from([(document, "Budget 2026".to_owned())]);

        let (text, links) = titled(&markdown, &titles, "Document");

        assert_eq!(text, "Summarise Budget 2026 please");
        assert_eq!(&text[links[0].range.clone()], "Budget 2026");
        assert_eq!(links[0].url, format!("koru://doc/{document}"));
        assert_eq!(documents(&markdown), vec![document]);
    }

    #[test]
    fn an_unknown_title_reads_as_the_fallback_until_it_arrives() {
        let document = DocumentId::new();
        let markdown = format!("[[koru://doc/{document}]]");
        let (text, links) = titled(&markdown, &HashMap::new(), "Document");
        assert_eq!(text, "Document");
        assert_eq!(links[0].range, 0..8);
    }

    #[test]
    fn plain_text_and_other_brackets_are_left_alone() {
        let (text, links) = titled("a [[b]] c", &HashMap::new(), "Document");
        assert_eq!(text, "a [[b]] c");
        assert!(links.is_empty());
    }
}
