// SPDX-License-Identifier: MPL-2.0

use super::*;
use rmcp::model::JsonObject;
use std::sync::Arc;

fn tool(name: &str, description_bytes: usize) -> Tool {
    Tool::new(
        name.to_owned(),
        "x".repeat(description_bytes),
        Arc::new(JsonObject::new()),
    )
}

fn walk(tools: &[Tool]) -> Vec<Vec<String>> {
    let mut pages = Vec::new();
    let mut cursor = None;
    loop {
        let page = page(tools.to_vec(), cursor.as_deref()).expect("a page");
        pages.push(page.tools.iter().map(|t| t.name.to_string()).collect());
        match page.next_cursor {
            Some(next) => cursor = Some(next),
            None => return pages,
        }
    }
}

#[test]
fn a_short_list_is_one_page_with_no_cursor() {
    let tools = vec![tool("a", 10), tool("b", 10)];
    let first = page(tools, None).unwrap();
    assert_eq!(first.tools.len(), 2);
    assert!(first.next_cursor.is_none());
}

#[test]
fn every_tool_is_on_exactly_one_page_and_each_page_fits() {
    let tools: Vec<Tool> = (0..30).map(|i| tool(&format!("t{i:02}"), 700)).collect();
    let pages = walk(&tools);
    assert!(pages.len() > 1, "{pages:?}");
    let flat: Vec<String> = pages.concat();
    let expected: Vec<String> = tools.iter().map(|t| t.name.to_string()).collect();
    assert_eq!(flat, expected);
    let mut cursor = None;
    for _ in &pages {
        let page = page(tools.clone(), cursor.as_deref()).unwrap();
        let bytes: usize = page.tools.iter().map(|t| encoded_len(t) + 1).sum();
        assert!(bytes + ENVELOPE_BYTES <= PAGE_BYTES, "{bytes}");
        cursor = page.next_cursor;
    }
}

#[test]
fn a_tool_larger_than_a_page_gets_a_page_of_its_own() {
    let tools = vec![tool("a", 100), tool("b", PAGE_BYTES * 2), tool("c", 100)];
    assert_eq!(walk(&tools), vec![vec!["a"], vec!["b"], vec!["c"]]);
}

#[test]
fn a_cursor_is_the_same_on_every_call() {
    let tools: Vec<Tool> = (0..30).map(|i| tool(&format!("t{i:02}"), 700)).collect();
    let once = page(tools.clone(), None).unwrap().next_cursor;
    let again = page(tools, None).unwrap().next_cursor;
    assert!(once.is_some());
    assert_eq!(once, again);
}

#[test]
fn a_cursor_this_server_did_not_hand_out_is_an_invalid_params_error() {
    let tools: Vec<Tool> = (0..30).map(|i| tool(&format!("t{i:02}"), 700)).collect();
    for cursor in [
        "",
        "2",
        "koru-tools:nope",
        "koru-tools:t00",
        "koru-tools:t01",
    ] {
        let error = page(tools.clone(), Some(cursor))
            .err()
            .unwrap_or_else(|| panic!("{cursor:?} was accepted"));
        assert_eq!(
            error.code,
            rmcp::model::ErrorCode::INVALID_PARAMS,
            "{cursor:?}"
        );
    }
}
