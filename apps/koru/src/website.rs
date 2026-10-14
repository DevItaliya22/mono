// SPDX-License-Identifier: MPL-2.0

//! App-owned marketing destinations share the existing website origin setting.
use koru_i18n::Lang;

pub const DEFAULT_ORIGIN: &str = "https://www.koru.do";
pub fn segment_for(lang: Lang) -> &'static str {
    match lang {
        Lang::En => "en",
        Lang::Es => "es",
        Lang::PtBr => "pt",
        Lang::De => "de",
        Lang::Fr => "fr",
        Lang::Ar => "ar",
        Lang::ZhHans => "zh",
        Lang::Ja => "ja",
    }
}
pub fn origin() -> String {
    origin_from(koru_env::value(koru_env::name::KORU_SUPPORT_URL).as_deref())
        .unwrap_or_else(|| DEFAULT_ORIGIN.to_string())
}
/// Preserve the existing support-origin contract, including stripping paths.
pub fn origin_from(value: Option<&str>) -> Option<String> {
    let raw = value?.trim();
    if raw.is_empty() {
        return None;
    }
    let (scheme, rest) = raw.split_once("://")?;
    if !(scheme.eq_ignore_ascii_case("http") || scheme.eq_ignore_ascii_case("https")) {
        return None;
    }
    let host = rest.split(['/', '?', '#']).next()?;
    if host.is_empty() {
        return None;
    }
    Some(format!("{}://{host}", scheme.to_ascii_lowercase()))
}
#[derive(Clone, Copy)]
pub enum Page {
    Pricing,
    Dashboard,
}
pub fn page_url(origin: &str, lang: Lang, page: Page) -> String {
    format!(
        "{origin}/{}/{}",
        segment_for(lang),
        match page {
            Page::Pricing => "pricing",
            Page::Dashboard => "dashboard",
        }
    )
}
