// SPDX-License-Identifier: MPL-2.0

//! Deterministic, self-contained human viewers for generated public contracts.
//!
//! They deliberately contain no script, fetch, credential field, or external
//! asset. The route/stream cards and the complete embedded JSON are projections
//! of the same generated documents the Gateway serves.

use serde_json::Value;

pub fn openapi(document: &Value) -> String {
    let mut cards = Vec::new();
    if let Some(paths) = document["paths"].as_object() {
        for (path, methods) in paths {
            let Some(methods) = methods.as_object() else {
                continue;
            };
            for (method, operation) in methods {
                if !matches!(method.as_str(), "get" | "post" | "put" | "delete" | "patch") {
                    continue;
                }
                let summary = operation["summary"].as_str().unwrap_or("Public operation");
                let scopes = operation["security"]
                    .as_array()
                    .and_then(|security| security.first())
                    .and_then(|requirement| requirement["bearerAuth"].as_array())
                    .map(|scopes| {
                        scopes
                            .iter()
                            .filter_map(Value::as_str)
                            .collect::<Vec<_>>()
                            .join(", ")
                    })
                    .filter(|scopes| !scopes.is_empty())
                    .unwrap_or_else(|| "Public discovery".into());
                cards.push((path.as_str(), method.to_ascii_uppercase(), summary, scopes));
            }
        }
    }
    cards.sort_by(|left, right| (left.0, &left.1).cmp(&(right.0, &right.1)));
    let mut body = String::new();
    for (path, method, summary, scopes) in cards {
        body.push_str("<article class=card><div class=operation><span class=method>");
        escape_into(&method, &mut body);
        body.push_str("</span><code>");
        escape_into(path, &mut body);
        body.push_str("</code></div><h2>");
        escape_into(summary, &mut body);
        body.push_str("</h2><p class=meta>Scope: ");
        escape_into(&scopes, &mut body);
        body.push_str("</p></article>");
    }
    page(
        "Koru API",
        "HTTP reference",
        "Generated from Koru’s public OpenAPI catalog. This file is complete offline and contains no credentials.",
        "/docs/asyncapi",
        "Streaming reference",
        &body,
        document,
    )
}

pub fn asyncapi(document: &Value) -> String {
    let mut cards = Vec::new();
    if let Some(channels) = document["channels"].as_object() {
        for (path, channel) in channels {
            let protocol = channel["x-protocol"].as_str().unwrap_or("stream");
            let ordered = channel["x-ordered"].as_bool().unwrap_or(false);
            let backpressured = channel["x-backpressured"].as_bool().unwrap_or(false);
            let messages = channel["messages"]
                .as_object()
                .map(|messages| messages.keys().cloned().collect::<Vec<_>>().join(", "))
                .unwrap_or_default();
            cards.push((path.as_str(), protocol, ordered, backpressured, messages));
        }
    }
    cards.sort_by_key(|card| card.0);
    let mut body = String::new();
    for (path, protocol, ordered, backpressured, messages) in cards {
        body.push_str("<article class=card><div class=operation><span class=method>");
        escape_into(&protocol.to_ascii_uppercase(), &mut body);
        body.push_str("</span><code>");
        escape_into(path, &mut body);
        body.push_str("</code></div><h2>");
        escape_into(
            if messages.is_empty() {
                "Streaming channel"
            } else {
                &messages
            },
            &mut body,
        );
        body.push_str("</h2><p class=meta>");
        body.push_str(if ordered { "Ordered" } else { "Unordered" });
        body.push_str(" · ");
        body.push_str(if backpressured {
            "Backpressured"
        } else {
            "Unbounded by contract"
        });
        body.push_str("</p></article>");
    }
    page(
        "Koru streams",
        "Streaming reference",
        "Generated from Koru’s public AsyncAPI catalog. Session streams use explicit independent cursors and gap records.",
        "/docs",
        "HTTP reference",
        &body,
        document,
    )
}

fn page(
    title: &str,
    eyebrow: &str,
    introduction: &str,
    sibling_href: &str,
    sibling_label: &str,
    cards: &str,
    document: &Value,
) -> String {
    let mut output = String::from(
        r#"<!doctype html><html lang=en><head><meta charset=utf-8><meta name=viewport content="width=device-width,initial-scale=1"><meta name=color-scheme content="light dark"><title>"#,
    );
    escape_into(title, &mut output);
    output.push_str(
        r#"</title><style>:root{font-family:Inter,"Segoe UI",system-ui,sans-serif;color-scheme:light dark;--bg:#f7f7f5;--surface:#fff;--text:#1f211f;--muted:#696d69;--line:#dedfda;--accent:#216e58;--code:#f0f1ed}*{box-sizing:border-box}body{margin:0;background:var(--bg);color:var(--text);font-size:14px;line-height:1.5}main{width:min(1040px,calc(100% - 32px));margin:0 auto;padding:48px 0 64px}header{display:grid;gap:12px;margin-bottom:32px}.eyebrow{margin:0;color:var(--accent);font-size:12px;font-weight:650;letter-spacing:.08em;text-transform:uppercase}h1{margin:0;font-size:32px;line-height:1.2;letter-spacing:-.02em}header p{max-width:680px;margin:0;color:var(--muted);font-size:15px}.nav{color:var(--accent);font-weight:600;text-underline-offset:3px}.grid{display:grid;grid-template-columns:repeat(auto-fit,minmax(280px,1fr));gap:12px}.card{min-width:0;padding:16px;border:1px solid var(--line);border-radius:12px;background:var(--surface)}.operation{display:flex;align-items:center;gap:8px;min-width:0}.method{flex:none;border-radius:6px;background:var(--code);padding:2px 6px;color:var(--accent);font-size:11px;font-weight:700;letter-spacing:.04em}code{overflow-wrap:anywhere;font-family:"JetBrains Mono","Cascadia Code",monospace;font-size:12px}h2{margin:12px 0 4px;font-size:15px;line-height:1.4}.meta{margin:0;color:var(--muted);font-size:12px}details{margin-top:32px;border-top:1px solid var(--line);padding-top:20px}summary{cursor:pointer;color:var(--accent);font-weight:650}pre{overflow:auto;max-height:70vh;margin:12px 0 0;padding:16px;border:1px solid var(--line);border-radius:12px;background:var(--surface);font:12px/1.55 "JetBrains Mono","Cascadia Code",monospace}@media(prefers-color-scheme:dark){:root{--bg:#181a18;--surface:#222522;--text:#eef0ec;--muted:#a8ada7;--line:#3a3e3a;--accent:#76c8a9;--code:#2c322e}}@media(max-width:520px){main{width:min(100% - 24px,1040px);padding-top:32px}h1{font-size:28px}}</style></head><body><main><header><p class=eyebrow>"#,
    );
    escape_into(eyebrow, &mut output);
    output.push_str("</p><h1>");
    escape_into(title, &mut output);
    output.push_str("</h1><p>");
    escape_into(introduction, &mut output);
    output.push_str("</p><a class=nav href=\"");
    escape_into(sibling_href, &mut output);
    output.push_str("\">");
    escape_into(sibling_label, &mut output);
    output.push_str("</a></header><section class=grid aria-label=\"Catalog entries\">");
    output.push_str(cards);
    output.push_str("</section><details><summary>Complete generated contract</summary><pre>");
    let json = serde_json::to_string_pretty(document).expect("public contract serializes");
    escape_into(&json, &mut output);
    output.push_str("</pre></details></main></body></html>\n");
    output
}

fn escape_into(value: &str, output: &mut String) {
    for character in value.chars() {
        match character {
            '&' => output.push_str("&amp;"),
            '<' => output.push_str("&lt;"),
            '>' => output.push_str("&gt;"),
            '"' => output.push_str("&quot;"),
            '\'' => output.push_str("&#39;"),
            _ => output.push(character),
        }
    }
}

#[cfg(test)]
mod tests {
    #[test]
    fn viewer_is_self_contained_and_escapes_catalog_text() {
        let document = serde_json::json!({
            "paths": {"/<unsafe>": {"get": {"summary": "A & B", "security": []}}}
        });
        let viewer = super::openapi(&document);
        assert!(viewer.contains("/&lt;unsafe&gt;"));
        assert!(viewer.contains("A &amp; B"));
        assert!(!viewer.contains("<script"));
        assert!(!viewer.contains("fetch("));
        assert!(!viewer.contains("sessionStorage"));
    }
}
