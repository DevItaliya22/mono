// SPDX-License-Identifier: MPL-2.0

use super::*;

fn support_policy(url: &str) -> Navigation {
    // The shape the app uses: its own origin and the Messenger's are allowed,
    // any other http(s) goes to the browser, anything else is refused.
    if url.starts_with("https://www.koru.do/") || url.starts_with("https://widget.intercom.io/") {
        Navigation::Allow
    } else if url.starts_with("http://") || url.starts_with("https://") {
        Navigation::External
    } else {
        Navigation::Refuse
    }
}

#[test]
fn policy_dispatch_allows_externalises_and_refuses() {
    assert_eq!(
        decide(&support_policy, "https://www.koru.do/en/support/desktop"),
        (true, None)
    );
    assert_eq!(
        decide(&support_policy, "https://example.com/article"),
        (
            false,
            Some(WebEvent::External {
                url: "https://example.com/article".into()
            })
        )
    );
    assert_eq!(
        decide(&support_policy, "javascript:alert(1)"),
        (
            false,
            Some(WebEvent::Refused {
                url: "javascript:alert(1)".into()
            })
        )
    );
    assert_eq!(
        decide(&support_policy, "file:///C:/Windows"),
        (
            false,
            Some(WebEvent::Refused {
                url: "file:///C:/Windows".into()
            })
        )
    );
}

#[test]
fn an_ipc_event_keeps_the_sender_uri_beside_the_body() {
    // The engine builds the event from wry's Request; the shape is what the
    // app validates against its origin, so it is pinned here.
    let event = WebEvent::Ipc {
        body: r#"{"ready":true}"#.into(),
        source: "https://www.koru.do/en/support/desktop".into(),
    };
    let WebEvent::Ipc { body, source } = event else {
        panic!()
    };
    assert_eq!(host_of(&source), "www.koru.do");
    assert!(body.contains("ready"));
}

#[test]
fn host_of_strips_scheme_path_query_and_fragment() {
    assert_eq!(
        host_of("https://api-iam.intercom.io/messenger/web/ping?x=1"),
        "api-iam.intercom.io"
    );
    assert_eq!(
        host_of("http://localhost:3000/en/support/desktop"),
        "localhost:3000"
    );
    assert_eq!(
        host_of("https://widget.intercom.io#frag"),
        "widget.intercom.io"
    );
    assert_eq!(host_of("about:blank"), "about:blank");
}

#[test]
fn a_js_string_literal_cannot_escape_the_literal() {
    let hostile = "\"; alert(1); //\n</script>\u{2028}\\";
    let literal = js_string_literal(hostile);
    assert!(literal.starts_with('"') && literal.ends_with('"'));
    let inner = &literal[1..literal.len() - 1];
    // No raw newline, angle bracket or line terminator survives, and every
    // quote inside is escaped.
    assert!(!inner.contains('\n'));
    assert!(!inner.contains('<'));
    assert!(!inner.contains('\u{2028}'));
    let bytes = inner.as_bytes();
    for (i, b) in bytes.iter().enumerate() {
        if *b == b'"' {
            assert_eq!(bytes[i - 1], b'\\', "unescaped quote at {i} in {inner}");
        }
    }
    assert_eq!(js_string_literal("plain"), "\"plain\"");
    assert_eq!(js_string_literal("a\"b"), "\"a\\\"b\"");
}

#[test]
fn the_fault_switch_only_knows_create() {
    assert_eq!(Fault::parse(Some("create")), Some(Fault::Create));
    assert_eq!(Fault::parse(Some(" create ")), Some(Fault::Create));
    assert_eq!(Fault::parse(Some("reparent")), None);
    assert_eq!(Fault::parse(None), None);
}

#[test]
fn the_error_display_names_the_stage() {
    assert!(
        WebError::RuntimeUnavailable("x".into())
            .to_string()
            .contains("no web runtime")
    );
    assert!(WebError::Create("x".into()).to_string().contains("create"));
    assert!(WebError::Call("x".into()).to_string().contains("call"));
}
