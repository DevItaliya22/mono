// SPDX-License-Identifier: MPL-2.0

//! Unicode emoji catalog, background search, and process-local usage ranking.
//!
//! Unicode strings remain ordinary document text. This module only supplies
//! metadata and ranking; it deliberately has no custom-asset representation.
//!
//! The catalog lists **default skin tones only**. Tone variants are reached at
//! long-press time via [`skin_tones_of`]; a single preferred tone is applied on
//! insert so the grid stays one cell per emoji rather than a wall of near-
//! identical faces.

use std::collections::HashMap;
use std::sync::{Arc, Mutex, OnceLock};

use emojis::SkinTone;
use nucleo::pattern::{CaseMatching, Normalization};
use nucleo::{Config, Nucleo, Utf32String};

#[derive(Clone, Debug)]
pub struct EmojiRecord {
    pub value: &'static str,
    pub name: &'static str,
    pub shortcode: Option<&'static str>,
    pub shortcodes: String,
    pub aliases: Vec<String>,
    pub category: emojis::Group,
    search_text: String,
}

impl EmojiRecord {
    pub fn category_name(&self) -> &'static str {
        group_name(self.category)
    }
}

pub fn group_name(group: emojis::Group) -> &'static str {
    match group {
        emojis::Group::SmileysAndEmotion => "Smileys & Emotion",
        emojis::Group::PeopleAndBody => "People & Body",
        emojis::Group::AnimalsAndNature => "Animals & Nature",
        emojis::Group::FoodAndDrink => "Food & Drink",
        emojis::Group::TravelAndPlaces => "Travel & Places",
        emojis::Group::Activities => "Activities",
        emojis::Group::Objects => "Objects",
        emojis::Group::Symbols => "Symbols",
        emojis::Group::Flags => "Flags",
    }
}

pub fn catalog() -> &'static [EmojiRecord] {
    static CATALOG: OnceLock<Vec<EmojiRecord>> = OnceLock::new();
    CATALOG.get_or_init(|| {
        let aliases: HashMap<String, Vec<String>> =
            serde_json::from_str(include_str!("../../../assets/emoji/aliases.json"))
                .expect("checked-in emoji aliases are valid JSON");
        let mut records = Vec::new();
        // Defaults only — `emojis::iter` already filters out non-default tones.
        // Expanding every Fitzpatrick variant here filled People & Body with
        // near-identical faces; tones are offered on long-press instead.
        for emoji in emojis::iter() {
            let shortcodes = emoji.shortcodes().collect::<Vec<_>>().join(" ");
            let extra = aliases.get(emoji.as_str()).cloned().unwrap_or_default();
            let category = emoji.group();
            let search_text = format!(
                "{} {} {} {}",
                emoji.name(),
                shortcodes,
                extra.join(" "),
                group_name(category)
            );
            records.push(EmojiRecord {
                value: emoji.as_str(),
                name: emoji.name(),
                shortcode: emoji.shortcode(),
                shortcodes,
                aliases: extra,
                category,
                search_text,
            });
        }
        records
    })
}

/// Skin-tone variants of a catalog (or any) emoji value, including the default.
/// Empty when the emoji does not take a tone.
pub fn skin_tones_of(value: &str) -> Vec<&'static str> {
    let Some(emoji) = emojis::get(value) else {
        return Vec::new();
    };
    let Some(tones) = emoji.skin_tones() else {
        return Vec::new();
    };
    tones.map(|e| e.as_str()).collect()
}

/// Whether this emoji has more than one skin-tone form.
pub fn has_skin_tones(value: &str) -> bool {
    skin_tones_of(value).len() > 1
}

/// Apply the process-preferred primary skin tone when the emoji supports it.
pub fn with_preferred_tone(value: &str) -> String {
    let tone = preferred_skin_tone();
    if tone == SkinTone::Default {
        return value.to_owned();
    }
    let Some(emoji) = emojis::get(value) else {
        return value.to_owned();
    };
    emoji
        .with_skin_tone(tone)
        .map(|e| e.as_str().to_owned())
        .unwrap_or_else(|| value.to_owned())
}

/// Primary tones offered as a remembered preference (not the 26-way couple matrix).
pub const PRIMARY_TONES: [SkinTone; 6] = [
    SkinTone::Default,
    SkinTone::Light,
    SkinTone::MediumLight,
    SkinTone::Medium,
    SkinTone::MediumDark,
    SkinTone::Dark,
];

pub fn skin_tone_token(tone: SkinTone) -> &'static str {
    match tone {
        SkinTone::Default => "default",
        SkinTone::Light => "light",
        SkinTone::MediumLight => "medium-light",
        SkinTone::Medium => "medium",
        SkinTone::MediumDark => "medium-dark",
        SkinTone::Dark => "dark",
        // Combination tones are insert-only from the long-press strip; they are
        // never the stored preference.
        _ => "default",
    }
}

pub fn parse_skin_tone(token: &str) -> SkinTone {
    match token.trim() {
        "light" => SkinTone::Light,
        "medium-light" => SkinTone::MediumLight,
        "medium" => SkinTone::Medium,
        "medium-dark" => SkinTone::MediumDark,
        "dark" => SkinTone::Dark,
        _ => SkinTone::Default,
    }
}

/// The primary tone of a concrete emoji value, when it is one of the six.
pub fn primary_tone_of(value: &str) -> Option<SkinTone> {
    let tone = emojis::get(value)?.skin_tone()?;
    PRIMARY_TONES.contains(&tone).then_some(tone)
}

fn tone_state() -> &'static Mutex<SkinTone> {
    static TONE: OnceLock<Mutex<SkinTone>> = OnceLock::new();
    TONE.get_or_init(|| Mutex::new(SkinTone::Default))
}

/// Serialises tests that flip the preferred tone — same race the theme
/// `AtomicBool` has under cargo's parallel runner.
#[cfg(test)]
pub fn tone_test_lock() -> std::sync::MutexGuard<'static, ()> {
    static LOCK: OnceLock<Mutex<()>> = OnceLock::new();
    LOCK.get_or_init(|| Mutex::new(()))
        .lock()
        .unwrap_or_else(|e| e.into_inner())
}

pub fn preferred_skin_tone() -> SkinTone {
    *tone_state().lock().unwrap_or_else(|e| e.into_inner())
}

pub fn set_preferred_skin_tone(tone: SkinTone) {
    let tone = if PRIMARY_TONES.contains(&tone) {
        tone
    } else {
        SkinTone::Default
    };
    *tone_state().lock().unwrap_or_else(|e| e.into_inner()) = tone;
}

/// Remember a tone chosen from the long-press strip when it is a primary tone.
pub fn remember_tone_from_value(value: &str) {
    if let Some(tone) = primary_tone_of(value) {
        set_preferred_skin_tone(tone);
    }
}

/// High-level nucleo worker. Candidate scoring runs on its rayon pool; the UI
/// only ticks briefly and reads an immutable completed snapshot.
pub struct EmojiSearch {
    worker: Nucleo<usize>,
    query: String,
}

impl EmojiSearch {
    pub fn new() -> Self {
        let mut worker = Nucleo::new(Config::DEFAULT, Arc::new(|| {}), None, 1);
        let injector = worker.injector();
        for (index, _) in catalog().iter().enumerate() {
            injector.push(index, |index, columns| {
                columns[0] = Utf32String::from(catalog()[*index].search_text.as_str());
            });
        }
        drop(injector);
        worker.tick(10);
        Self {
            worker,
            query: String::new(),
        }
    }

    pub fn set_query(&mut self, query: &str) {
        let query = query.trim().to_lowercase();
        if query == self.query {
            return;
        }
        let append = query.starts_with(&self.query);
        self.worker.pattern.reparse(
            0,
            &query,
            CaseMatching::Ignore,
            Normalization::Smart,
            append,
        );
        self.query = query;
        self.worker.tick(0);
    }

    pub fn results(&mut self, limit: usize) -> Vec<usize> {
        // An empty query matches the complete catalog without scoring. The
        // worker's first snapshot can still be empty when a picker opens;
        // waiting longer here would make UI responsiveness depend on its pool.
        let mut hits: Vec<usize> = if self.query.is_empty() {
            (0..catalog().len()).take(limit.max(64)).collect()
        } else {
            self.worker.tick(3);
            let snapshot = self.worker.snapshot();
            let count = snapshot.matched_item_count().min(limit.max(64) as u32);
            snapshot
                .matched_items(..count)
                .map(|item| *item.data)
                .collect()
        };
        let query = self.query.as_str();
        // `sort_by_cached_key`, not `sort_by_key`: the key is computed once per
        // element rather than once per comparison. That is the cheaper call —
        // `rank_key` walks every shortcode of a record — but it is also the
        // *correct* one, because the key is not a pure function of the record.
        // It reads `usage()`, a process-global another thread may be writing
        // through `record_used`, and a key that changes mid-sort is not a total
        // order: the standard library detects that and panics. It did, under
        // cargo's parallel runner, roughly one run in five.
        hits.sort_by_cached_key(|&index| rank_key(&catalog()[index], query, index));
        hits.truncate(limit);
        hits
    }
}

fn rank_key(record: &EmojiRecord, query: &str, catalog_index: usize) -> (u8, u8, usize) {
    if query.is_empty() {
        let recent = usage()
            .lock()
            .unwrap()
            .iter()
            .position(|v| v == record.value);
        return (
            recent.is_none() as u8,
            recent.unwrap_or(u8::MAX as usize).min(u8::MAX as usize) as u8,
            catalog_index,
        );
    }
    let exact_shortcode = record.shortcodes.split_whitespace().any(|s| s == query);
    let exact_name = record.name.eq_ignore_ascii_case(query);
    let prefix = record.name.starts_with(query)
        || record
            .shortcodes
            .split_whitespace()
            .any(|s| s.starts_with(query));
    let alias = record
        .aliases
        .iter()
        .any(|a| a == query || a.starts_with(query));
    let tier = if exact_shortcode {
        0
    } else if exact_name {
        1
    } else if prefix {
        2
    } else if alias {
        3
    } else {
        4
    };
    let recent = usage()
        .lock()
        .unwrap()
        .iter()
        .position(|v| v == record.value)
        .unwrap_or(u8::MAX as usize)
        .min(u8::MAX as usize) as u8;
    (tier, recent, catalog_index)
}

fn usage() -> &'static Mutex<Vec<String>> {
    static USAGE: OnceLock<Mutex<Vec<String>>> = OnceLock::new();
    USAGE.get_or_init(|| Mutex::new(Vec::new()))
}

pub fn record_used(value: &str) {
    let mut recent = usage().lock().unwrap();
    recent.retain(|item| item != value);
    recent.insert(0, value.to_owned());
    recent.truncate(32);
}

pub fn recent_usage() -> Vec<String> {
    usage().lock().unwrap().clone()
}

pub fn restore_usage(values: &[String]) {
    let mut recent = usage().lock().unwrap();
    *recent = values
        .iter()
        .filter(|value| emojis::get(value).is_some())
        .take(32)
        .cloned()
        .collect();
}

#[cfg(test)]
mod readiness_tests;

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn catalog_comes_from_unicode_data_and_includes_aliases() {
        let rocket = catalog().iter().find(|e| e.value == "🚀").unwrap();
        assert_eq!(rocket.shortcode, Some("rocket"));
        assert!(rocket.aliases.iter().any(|a| a == "launch"));
        assert!(catalog().len() > 1_000);
    }

    #[test]
    fn catalog_lists_defaults_only() {
        assert!(catalog().iter().any(|e| e.value == "👍"));
        assert!(
            !catalog().iter().any(|e| e.value == "👍🏽"),
            "tone variants live on long-press, not in the grid"
        );
        let tones = skin_tones_of("👍");
        assert!(tones.len() > 1);
        assert!(tones.contains(&"👍🏽"));
    }

    #[test]
    fn preferred_tone_is_applied_when_supported() {
        let _guard = tone_test_lock();
        set_preferred_skin_tone(SkinTone::Medium);
        assert_eq!(with_preferred_tone("👍"), "👍🏽");
        assert_eq!(with_preferred_tone("🚀"), "🚀");
        set_preferred_skin_tone(SkinTone::Default);
    }

    #[test]
    fn remembering_a_tone_updates_the_preference() {
        let _guard = tone_test_lock();
        set_preferred_skin_tone(SkinTone::Default);
        remember_tone_from_value("👍🏾");
        assert_eq!(preferred_skin_tone(), SkinTone::MediumDark);
        set_preferred_skin_tone(SkinTone::Default);
    }

    #[test]
    fn skin_tone_tokens_round_trip() {
        for tone in PRIMARY_TONES {
            assert_eq!(parse_skin_tone(skin_tone_token(tone)), tone);
        }
        assert_eq!(parse_skin_tone("nope"), SkinTone::Default);
    }

    #[test]
    fn exact_shortcodes_and_product_aliases_rank_first() {
        let mut search = EmojiSearch::new();
        search.set_query("rocket");
        assert_eq!(catalog()[search.results(10)[0]].value, "🚀");
        search.set_query("approved");
        assert_eq!(catalog()[search.results(10)[0]].value, "✅");
    }
}
