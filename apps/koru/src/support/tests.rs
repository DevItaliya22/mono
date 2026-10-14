// SPDX-License-Identifier: MPL-2.0

use super::url::*;
use koru_i18n::Lang;
use koru_webview::Navigation;
use std::path::PathBuf;

#[test]
fn every_language_has_a_segment_and_the_page_url_uses_it() {
    let rows = [
        (Lang::En, "en"),
        (Lang::Es, "es"),
        (Lang::PtBr, "pt"),
        (Lang::De, "de"),
        (Lang::Fr, "fr"),
        (Lang::Ar, "ar"),
        (Lang::ZhHans, "zh"),
        (Lang::Ja, "ja"),
    ];
    for (lang, segment) in rows {
        assert_eq!(segment_for(lang), segment);
        assert_eq!(
            page_url("https://www.koru.do", lang),
            format!("https://www.koru.do/{segment}/support/desktop")
        );
    }
    assert_eq!(
        page_url("http://localhost:3000", Lang::PtBr),
        "http://localhost:3000/pt/support/desktop"
    );
}

/// The default is the site, and it survives the same normalisation an
/// override goes through.
#[test]
fn the_default_origin_is_the_site() {
    assert_eq!(
        origin_from(Some(super::url::DEFAULT_ORIGIN)).as_deref(),
        Some(super::url::DEFAULT_ORIGIN)
    );
}

#[test]
fn the_support_url_is_an_origin() {
    assert_eq!(origin_from(None), None);
    assert_eq!(origin_from(Some("")), None);
    assert_eq!(origin_from(Some("  ")), None);
    assert_eq!(
        origin_from(Some("https://www.koru.do")).as_deref(),
        Some("https://www.koru.do")
    );
    assert_eq!(
        origin_from(Some("https://www.koru.do/")).as_deref(),
        Some("https://www.koru.do")
    );
    assert_eq!(
        origin_from(Some("HTTP://localhost:3000/en/support/desktop")).as_deref(),
        Some("http://localhost:3000")
    );
    assert_eq!(origin_from(Some("ftp://x")), None);
    assert_eq!(origin_from(Some("www.koru.do")), None);
}

#[test]
fn navigation_stays_inside_for_the_support_and_intercom_origins() {
    let o = "https://www.koru.do";
    assert_eq!(
        navigation(o, "https://www.koru.do/en/support/desktop"),
        Navigation::Allow
    );
    assert_eq!(navigation(o, "https://www.koru.do"), Navigation::Allow);
    assert_eq!(
        navigation(o, "https://www.koru.do.evil.example/x"),
        Navigation::External
    );
    assert_eq!(
        navigation(o, "https://widget.intercom.io/widget/xerpcuus"),
        Navigation::Allow
    );
    assert_eq!(
        navigation(o, "https://api-iam.intercom.io/messenger/web/ping"),
        Navigation::Allow
    );
    assert_eq!(
        navigation(o, "https://example.com/article"),
        Navigation::External
    );
    assert_eq!(
        navigation(o, "http://localhost:3000/en"),
        Navigation::External
    );
    assert_eq!(navigation(o, "javascript:alert(1)"), Navigation::Refuse);
    assert_eq!(navigation(o, "file:///C:/"), Navigation::Refuse);
    assert_eq!(
        navigation(o, "http://intercom.io/"),
        Navigation::External,
        "not https"
    );
}

#[test]
fn ipc_is_believed_only_from_the_support_origin() {
    let o = "https://www.koru.do";
    assert!(same_origin_for_ipc(
        o,
        "https://www.koru.do/en/support/desktop"
    ));
    assert!(!same_origin_for_ipc(o, "https://widget.intercom.io/frame"));
    assert!(!same_origin_for_ipc(o, "https://www.koru.do.evil.example/"));
}

#[test]
fn the_data_directory_is_under_the_root_when_there_is_one() {
    assert_eq!(
        data_directory_from(Some(PathBuf::from("D:/koru-data/store")), None),
        PathBuf::from("D:/koru-data/store")
            .join("webview")
            .join("support")
    );
    assert_eq!(
        data_directory_from(None, Some(PathBuf::from("C:/Users/x/AppData/Local"))),
        PathBuf::from("C:/Users/x/AppData/Local")
            .join("Koru")
            .join("webview")
            .join("support")
    );
}
