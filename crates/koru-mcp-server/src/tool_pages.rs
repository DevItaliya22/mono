// SPDX-License-Identifier: MPL-2.0

//! `tools/list` served in pages, each one line under [`PAGE_BYTES`].
//!
//! Grok CLI 1.0.34 loses an 8,192-byte chunk of a longer line it reads from a
//! stdio server, and the compacted list of 21 typed tools is about 17 KB
//! (ADR-433-compacttools). MCP pages a list with an opaque `nextCursor` that
//! the client sends back until none is returned, so a list served in pages
//! that each fit under 8 KiB never meets that defect.
//!
//! **Pages are packed from the router's own sorted list**, greedily, so the
//! same tool set always pages the same way: a cursor is stable across calls and
//! across processes serving the same build. A cursor names where its page
//! starts, and the whole string is opaque to the client; one this server did
//! not hand out is an `invalid_params` error, as the protocol asks, never a
//! silent page one — a client that restarted on page one would list some
//! tools twice.

use rmcp::{ErrorData, model::Tool};

/// The most one `tools/list` response line may take, envelope included.
/// Under Grok's 8,192-byte read, with room for a JSON-RPC id and the cache
/// hints a newer protocol adds.
pub(crate) const PAGE_BYTES: usize = 7_000;

/// What a response line spends outside its tools: the JSON-RPC envelope,
/// `nextCursor` and its value, `resultType` and the cache hints.
const ENVELOPE_BYTES: usize = 320;

const CURSOR_PREFIX: &str = "koru-tools:";

/// One page of the list, and the cursor of the next, if there is one.
pub(crate) struct Page {
    pub tools: Vec<Tool>,
    pub next_cursor: Option<String>,
}

fn encoded_len(tool: &Tool) -> usize {
    serde_json::to_vec(tool).map_or(usize::MAX, |bytes| bytes.len())
}

/// Where each page starts, as indices into `tools`. A tool larger than a page
/// on its own gets a page of its own rather than being dropped.
fn page_starts(tools: &[Tool]) -> Vec<usize> {
    let mut starts = Vec::new();
    let mut used = 0;
    for (index, tool) in tools.iter().enumerate() {
        let size = encoded_len(tool).saturating_add(1);
        if starts.is_empty() || used + size > PAGE_BYTES - ENVELOPE_BYTES {
            starts.push(index);
            used = 0;
        }
        used += size;
    }
    starts
}

fn cursor_for(tool: &Tool) -> String {
    format!("{CURSOR_PREFIX}{}", tool.name)
}

/// The page `cursor` names in `tools`, which must be the router's sorted list.
pub(crate) fn page(tools: Vec<Tool>, cursor: Option<&str>) -> Result<Page, ErrorData> {
    let starts = page_starts(&tools);
    let position = match cursor {
        None => 0,
        Some(cursor) => starts
            .iter()
            .position(|&start| cursor_for(&tools[start]) == cursor)
            .filter(|&position| position > 0)
            .ok_or_else(|| {
                ErrorData::invalid_params(
                    format!("unknown tools/list cursor {cursor:?}; list again without one"),
                    None,
                )
            })?,
    };
    let start = starts.get(position).copied().unwrap_or(tools.len());
    let end = starts.get(position + 1).copied().unwrap_or(tools.len());
    let next_cursor = starts
        .get(position + 1)
        .map(|&next| cursor_for(&tools[next]));
    Ok(Page {
        tools: tools[start..end].to_vec(),
        next_cursor,
    })
}

#[cfg(test)]
mod tests;
