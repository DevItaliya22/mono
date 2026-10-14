// SPDX-License-Identifier: MPL-2.0

//! The chrome every page on `updates.koru.do` shares.
//!
//! There are two pages now — a release's notes and the index that lists them —
//! and they are uploaded months apart into a bucket with no build step, no
//! stylesheet object and no way to correct an immutable page once it is live.
//! A second hand-written `<style>` block would drift from the first at the
//! first colour change, and the drift would only ever be visible to whoever
//! happened to open both in one sitting. So the prelude is one function and the
//! stylesheet is one constant.
//!
//! **`STYLE` is byte-identical to what the notes page shipped with**, and the
//! index's own rules are appended through `head_extra` rather than merged in.
//! That is what lets the four page tests in `main.rs` keep passing unchanged,
//! and it keeps a page already in the bucket comparable with one rendered
//! today.

/// The shared stylesheet. Every colour in it is also in `docs/features/design/DESIGN.md`:
/// `#20201f` text, `#666` secondary, `#e0ddd8` borders, `#faf9f7` panels.
pub const STYLE: &str = "body{font:18px/1.6 system-ui,sans-serif;max-width:760px;margin:4rem auto;padding:0 1.5rem;color:#20201f}\
     h1{line-height:1.15}time{color:#666}\
     .get{margin:2rem 0;padding:1.25rem 1.5rem;border:1px solid #e0ddd8;border-radius:12px;background:#faf9f7}\
     .get a.zip{display:inline-block;font-weight:600;font-size:1.05em;text-decoration:none;color:#fff;background:#20201f;padding:.6rem 1.1rem;border-radius:8px}\
     .get p{margin:.75rem 0 0;font-size:.85em;color:#666}\
     .get code{font:.85em/1.5 ui-monospace,SFMono-Regular,Menlo,monospace;word-break:break-all}";

/// Wrap `body` in the shared document. `head` is appended to the `<style>`
/// block and `link` to the head itself — both usually empty.
pub fn shell(title: &str, style_extra: &str, link: &str, body: &str) -> String {
    format!(
        "<!doctype html><html lang=\"en\"><meta charset=\"utf-8\">\
         <meta name=\"viewport\" content=\"width=device-width,initial-scale=1\">\
         <title>{title}</title>{link}\
         <style>{STYLE}{style_extra}</style>\
         <main>{body}</main></html>"
    )
}

/// **Not a general-purpose sanitiser.** It escapes the five characters that
/// change the meaning of markup in an attribute or a text node, which is all
/// that is needed for the values these pages interpolate — titles, versions,
/// dates and summary lines. Do not reach for it to make arbitrary HTML safe.
pub fn escape_html(value: &str) -> String {
    value
        .replace('&', "&amp;")
        .replace('<', "&lt;")
        .replace('>', "&gt;")
        .replace('"', "&quot;")
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The whole reason this module exists. If someone adds a colour to one
    /// page by hand, the two stop sharing and this fails.
    #[test]
    fn both_pages_are_built_from_one_stylesheet() {
        // Built here rather than borrowed from `main.rs`'s `mod tests`: a
        // private helper is visible to descendants of the module that declares
        // it, and this module is that one's sibling.
        let download = crate::Download {
            file_name: "koru-windows-x86_64.zip",
            size: 45_088_768,
            sha256: &"a".repeat(64),
        };
        let notes = crate::Descriptor {
            title: "A release".into(),
            summary: vec!["One".into()],
            body: "Body.".into(),
        }
        .html("1.0.0", "today", &download);
        let index = crate::index::ReleaseIndex::default().html();

        assert!(
            notes.contains(STYLE),
            "the notes page dropped the stylesheet"
        );
        assert!(
            index.contains(STYLE),
            "the index page dropped the stylesheet"
        );
    }

    #[test]
    fn the_five_characters_that_change_markup_are_escaped() {
        assert_eq!(
            escape_html(r#"a<b>&"c"#),
            "a&lt;b&gt;&amp;&quot;c",
            "& must be replaced first, or the others double-escape"
        );
    }
}
