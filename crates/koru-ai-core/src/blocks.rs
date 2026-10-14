// SPDX-License-Identifier: MPL-2.0

//! Finalizing streamed Markdown into immutable deep-link and search units.

use crate::{MessageBlock, MessageBlockId, MessageBlockKind, MessageId};
use pulldown_cmark::{Event, Parser};

pub fn finalize_markdown(message_id: MessageId, markdown: &str) -> Vec<MessageBlock> {
    let lines: Vec<&str> = markdown.lines().collect();
    let mut blocks = Vec::new();
    let mut index = 0;
    while index < lines.len() {
        if lines[index].trim().is_empty() {
            index += 1;
            continue;
        }

        let (kind, block_markdown, plain_text, next) = if is_fence(lines[index]) {
            fenced_block(&lines, index)
        } else if let Some(plain) = heading_text(lines[index]) {
            (
                MessageBlockKind::Heading,
                lines[index].to_owned(),
                inline_plain_text(plain),
                index + 1,
            )
        } else if let Some(plain) = list_text(lines[index]) {
            (
                MessageBlockKind::ListItem,
                lines[index].to_owned(),
                inline_plain_text(plain),
                index + 1,
            )
        } else if let Some(plain) = lines[index].strip_prefix('>') {
            (
                MessageBlockKind::Quote,
                lines[index].to_owned(),
                inline_plain_text(plain.trim_start()),
                index + 1,
            )
        } else if is_rule(lines[index]) {
            (
                MessageBlockKind::Rule,
                lines[index].to_owned(),
                String::new(),
                index + 1,
            )
        } else {
            paragraph(&lines, index)
        };

        blocks.push(MessageBlock {
            id: MessageBlockId::new(),
            message_id,
            ordinal: blocks.len() as u32,
            kind,
            markdown: block_markdown,
            plain_text,
        });
        index = next;
    }
    blocks
}

fn paragraph(lines: &[&str], start: usize) -> (MessageBlockKind, String, String, usize) {
    let mut end = start + 1;
    while end < lines.len() && !lines[end].trim().is_empty() && !is_structural_start(lines[end]) {
        end += 1;
    }
    (
        MessageBlockKind::Paragraph,
        lines[start..end].join("\n"),
        inline_plain_text(&lines[start..end].join("\n")),
        end,
    )
}

fn inline_plain_text(markdown: &str) -> String {
    let mut plain = String::new();
    for event in Parser::new(markdown) {
        match event {
            Event::Text(text) | Event::Code(text) => plain.push_str(&text),
            Event::SoftBreak | Event::HardBreak => plain.push(' '),
            _ => {}
        }
    }
    plain
}

fn fenced_block(lines: &[&str], start: usize) -> (MessageBlockKind, String, String, usize) {
    let marker = lines[start].trim_start();
    let fence_char = marker.chars().next().unwrap_or('`');
    let mut end = start + 1;
    while end < lines.len() {
        if lines[end]
            .trim_start()
            .starts_with(&fence_char.to_string().repeat(3))
        {
            end += 1;
            break;
        }
        end += 1;
    }
    let plain_end = if end > start + 1 && is_fence(lines[end - 1]) {
        end - 1
    } else {
        end
    };
    (
        MessageBlockKind::Code,
        lines[start..end].join("\n"),
        lines[start + 1..plain_end].join("\n"),
        end,
    )
}

fn is_structural_start(line: &str) -> bool {
    is_fence(line)
        || heading_text(line).is_some()
        || list_text(line).is_some()
        || line.starts_with('>')
        || is_rule(line)
}

fn is_fence(line: &str) -> bool {
    let line = line.trim_start();
    line.starts_with("```") || line.starts_with("~~~")
}

fn heading_text(line: &str) -> Option<&str> {
    let line = line.trim_start();
    let hashes = line
        .chars()
        .take_while(|character| *character == '#')
        .count();
    (1..=6)
        .contains(&hashes)
        .then(|| line[hashes..].strip_prefix(' '))
        .flatten()
}

fn list_text(line: &str) -> Option<&str> {
    let line = line.trim_start();
    for prefix in ["- ", "* ", "+ "] {
        if let Some(text) = line.strip_prefix(prefix) {
            return Some(text);
        }
    }
    let digits = line.chars().take_while(char::is_ascii_digit).count();
    if digits > 0 {
        line[digits..].strip_prefix(". ")
    } else {
        None
    }
}

fn is_rule(line: &str) -> bool {
    matches!(line.trim(), "---" | "***" | "___")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn finalization_preserves_markdown_and_builds_search_text() {
        let message = MessageId::new();
        let markdown = "# **Heading**\n\nTwo *lines*\nin a [paragraph](https://example.com).\n\n- `item`\n\n```rust\nlet x = 1;\n```";
        let blocks = finalize_markdown(message, markdown);
        assert_eq!(
            blocks.iter().map(|block| block.kind).collect::<Vec<_>>(),
            vec![
                MessageBlockKind::Heading,
                MessageBlockKind::Paragraph,
                MessageBlockKind::ListItem,
                MessageBlockKind::Code,
            ]
        );
        assert_eq!(blocks[0].plain_text, "Heading");
        assert_eq!(blocks[1].plain_text, "Two lines in a paragraph.");
        assert_eq!(blocks[2].plain_text, "item");
        assert_eq!(blocks[3].plain_text, "let x = 1;");
        assert_eq!(blocks[3].markdown, "```rust\nlet x = 1;\n```");
        assert!(blocks.iter().all(|block| block.message_id == message));
        assert_eq!(
            blocks.iter().map(|block| block.ordinal).collect::<Vec<_>>(),
            vec![0, 1, 2, 3]
        );
    }

    #[test]
    fn empty_markdown_has_no_finalized_blocks() {
        assert!(finalize_markdown(MessageId::new(), "\n\n").is_empty());
    }
}
