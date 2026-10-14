// SPDX-License-Identifier: MPL-2.0

use unicode_segmentation::UnicodeSegmentation;
use uuid::Uuid;

use crate::{Network, Post};

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Issue {
    NoDestination,
    EmptyBody,
    MissingSubreddit,
    MissingRedditTitle,
    MissingFlair,
    TooLong,
    MissingTime,
    PastTime,
    AlreadyPublishing,
}

/// Validation is independent of connectivity: people can schedule offline.
pub fn validate(post: &Post, now_ms: i64) -> Vec<(Option<Uuid>, Issue)> {
    let mut issues = Vec::new();
    if !post.editable() {
        issues.push((None, Issue::AlreadyPublishing));
    }
    if post.destinations.is_empty() {
        issues.push((None, Issue::NoDestination));
    }
    if post.body.trim().is_empty() {
        issues.push((None, Issue::EmptyBody));
    }
    for destination in &post.destinations {
        let id = Some(destination.channel.id);
        let body = destination.body(&post.body);
        if body.trim().is_empty() && !post.body.trim().is_empty() {
            issues.push((id, Issue::EmptyBody));
        }
        match destination.channel.network {
            Network::X => {
                if destination
                    .parts(&post.body)
                    .iter()
                    .any(|p| weighted_length(p) > 280)
                {
                    issues.push((id, Issue::TooLong));
                }
            }
            Network::LinkedInPage | Network::LinkedInPerson => {
                if body.chars().count() > 3000 {
                    issues.push((id, Issue::TooLong));
                }
            }
            Network::Reddit => {
                let sub = destination.subreddit.trim().trim_start_matches("r/");
                if sub.is_empty() || !sub.chars().all(|c| c.is_ascii_alphanumeric() || c == '_') {
                    issues.push((id, Issue::MissingSubreddit));
                }
                if destination.reddit_title.trim().is_empty() {
                    issues.push((id, Issue::MissingRedditTitle));
                }
                if destination.reddit_title.chars().count() > 300 || body.chars().count() > 40_000 {
                    issues.push((id, Issue::TooLong));
                }
                if destination.flair_required
                    && destination.flair_id.as_deref().is_none_or(|v| v.is_empty())
                {
                    issues.push((id, Issue::MissingFlair));
                }
            }
        }
    }
    match post.scheduled_at_ms {
        None => issues.push((None, Issue::MissingTime)),
        Some(at) if at <= now_ms => issues.push((None, Issue::PastTime)),
        _ => {}
    }
    issues
}

/// Conservative X preview counter. URL tokens use the published 23-character
/// contribution; emoji clusters count as two. Provider validation is still
/// authoritative (full twitter-text URL grammar is a qualification gate).
pub fn weighted_length(text: &str) -> usize {
    text.split_inclusive(char::is_whitespace)
        .map(|token| {
            let trimmed = token.trim_end();
            let space = token[trimmed.len()..].chars().count();
            if trimmed.starts_with("https://") || trimmed.starts_with("http://") {
                23 + space
            } else {
                token
                    .graphemes(true)
                    .map(|g| {
                        if g.chars().any(|c| c as u32 >= 0x1f000) {
                            2
                        } else {
                            g.chars()
                                .map(|c| match c as u32 {
                                    0..=0x10ff
                                    | 0x2000..=0x200d
                                    | 0x2010..=0x201f
                                    | 0x2032..=0x2037 => 1,
                                    _ => 2,
                                })
                                .sum()
                        }
                    })
                    .sum::<usize>()
            }
        })
        .sum()
}

pub fn split_thread(body: &str) -> Vec<String> {
    let body = body.trim();
    if weighted_length(body) <= 280 {
        return vec![body.to_owned()];
    }
    // Whole URL tokens are indivisible and have a fixed weight. Every other
    // unit is a grapheme. Precompute once rather than recounting each prefix.
    let mut units = Vec::new();
    for token in body.split_inclusive(char::is_whitespace) {
        let text = token.trim_end();
        if text.starts_with("https://") || text.starts_with("http://") {
            units.push((text, 23));
            units.extend(
                token[text.len()..]
                    .graphemes(true)
                    .map(|g| (g, weighted_length(g))),
            );
        } else {
            units.extend(token.graphemes(true).map(|g| (g, weighted_length(g))));
        }
    }
    let mut result = Vec::new();
    let mut start = 0;
    while start < units.len() {
        let prefix = format!("{}/ ", result.len() + 1);
        let budget = 280 - weighted_length(&prefix);
        let mut end = start;
        let mut boundary = start;
        let mut sentence = start;
        let mut weight = 0;
        for (index, (grapheme, cost)) in units.iter().enumerate().skip(start) {
            if weight + cost > budget {
                break;
            }
            weight += cost;
            end = index + 1;
            if grapheme.chars().all(char::is_whitespace) {
                boundary = end;
                if index > start
                    && units[index - 1]
                        .0
                        .chars()
                        .last()
                        .is_some_and(|c| matches!(c, '.' | '!' | '?' | '。' | '！' | '？'))
                {
                    sentence = end;
                }
            }
        }
        let split = if end == units.len() || boundary == start {
            end
        } else if sentence > start {
            sentence
        } else {
            boundary
        };
        let split = if split == start {
            // A grapheme can carry arbitrarily many combining marks. Keep it
            // intact and let validation refuse an oversized part; never drop it.
            start + 1
        } else {
            split
        };
        let text: String = units[start..split].iter().map(|(text, _)| *text).collect();
        result.push(format!("{prefix}{}", text.trim()));
        start = split;
        while start < units.len() && units[start].0.chars().all(char::is_whitespace) {
            start += 1;
        }
    }
    result
}
