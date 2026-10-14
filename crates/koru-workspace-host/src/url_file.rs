// SPDX-License-Identifier: MPL-2.0

//! Shared links stored as workspace files.
//!
//! Android and iOS share a web URL as a file named `Shared link.url` whose
//! bytes are `text/uri-list` — the URL plus a newline, not a Windows Internet
//! Shortcut. Opening those bytes through the `.url` association asks Windows
//! for a `URL=` key that is not there, and the shell reports an empty target.
//!
//! Detect the file by name or MIME, read the URL out of either the uri-list
//! form or a real shortcut, and hand that URL to the desktop. Anything else
//! stays an ordinary file.

/// What to do with a file's bytes when the user asks to open it.
#[derive(Clone, PartialEq, Eq, Debug)]
pub enum OpenTarget {
    /// Open this URL in the desktop's default handler.
    Url(String),
    /// Named or typed as a link, but no usable URL was in the bytes.
    InvalidLink,
    /// Not a link: write a temp file and use the OS association.
    File,
}

/// Decide how to open a workspace file from its listing metadata and bytes.
pub fn open_target(name: &str, mime: Option<&str>, bytes: &[u8]) -> OpenTarget {
    if !is_url_file(name, mime) {
        return OpenTarget::File;
    }
    match parse_url(bytes) {
        Some(url) => OpenTarget::Url(url),
        None => OpenTarget::InvalidLink,
    }
}

fn is_url_file(name: &str, mime: Option<&str>) -> bool {
    mime_is_url(mime) || name_is_url(name)
}

fn mime_is_url(mime: Option<&str>) -> bool {
    mime.is_some_and(|mime| {
        mime.eq_ignore_ascii_case("text/uri-list")
            || mime.eq_ignore_ascii_case("application/internet-shortcut")
            || mime.eq_ignore_ascii_case("application/x-url")
    })
}

fn name_is_url(name: &str) -> bool {
    std::path::Path::new(name)
        .extension()
        .is_some_and(|ext| ext.eq_ignore_ascii_case("url"))
}

/// Pull a URL out of a uri-list, a Windows Internet Shortcut, or a bare URL.
fn parse_url(bytes: &[u8]) -> Option<String> {
    let text = std::str::from_utf8(bytes).ok()?.trim();
    if text.is_empty() {
        return None;
    }
    if let Some(url) = internet_shortcut_url(text) {
        return Some(url);
    }
    for line in text.lines() {
        let line = line.trim();
        if line.is_empty() || line.starts_with('#') {
            continue;
        }
        if looks_like_url(line) {
            return Some(line.to_owned());
        }
        break;
    }
    None
}

fn internet_shortcut_url(text: &str) -> Option<String> {
    let mut in_section = false;
    for line in text.lines() {
        let line = line.trim();
        if line.is_empty() {
            continue;
        }
        if line.starts_with('[') && line.ends_with(']') {
            in_section = line.eq_ignore_ascii_case("[InternetShortcut]");
            continue;
        }
        if !in_section {
            continue;
        }
        let Some((key, value)) = line.split_once('=') else {
            continue;
        };
        if key.trim().eq_ignore_ascii_case("URL") {
            let url = value.trim();
            if looks_like_url(url) {
                return Some(url.to_owned());
            }
        }
    }
    None
}

fn looks_like_url(s: &str) -> bool {
    let lower = s.to_ascii_lowercase();
    lower.starts_with("https://") || lower.starts_with("http://") || lower.starts_with("mailto:")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_shared_link_url_opens_the_url_inside() {
        assert_eq!(
            open_target(
                "Shared link.url",
                Some("text/uri-list"),
                b"https://example.com/article\n"
            ),
            OpenTarget::Url("https://example.com/article".into())
        );
    }

    #[test]
    fn a_windows_internet_shortcut_opens_its_url() {
        let bytes = b"[InternetShortcut]\r\nURL=https://koru.do/\r\n";
        assert_eq!(
            open_target("page.url", None, bytes),
            OpenTarget::Url("https://koru.do/".into())
        );
    }

    #[test]
    fn uri_list_comments_are_skipped() {
        let bytes = b"# this is a comment\nhttps://example.com\n";
        assert_eq!(
            open_target("Shared link.url", Some("text/uri-list"), bytes),
            OpenTarget::Url("https://example.com".into())
        );
    }

    #[test]
    fn an_empty_url_file_is_an_invalid_link() {
        assert_eq!(
            open_target("Shared link.url", Some("text/uri-list"), b"\n"),
            OpenTarget::InvalidLink
        );
    }

    #[test]
    fn a_pdf_is_still_a_file() {
        assert_eq!(
            open_target("Report.pdf", Some("application/pdf"), b"%PDF"),
            OpenTarget::File
        );
    }

    #[test]
    fn a_url_named_file_without_a_url_does_not_fall_through_to_the_os() {
        assert_eq!(
            open_target("Shared link.url", None, b"not a url"),
            OpenTarget::InvalidLink
        );
    }

    #[test]
    fn http_urls_are_accepted() {
        assert_eq!(
            parse_url(b"http://example.com/path"),
            Some("http://example.com/path".into())
        );
    }
}
