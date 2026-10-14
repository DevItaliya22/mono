// SPDX-License-Identifier: MPL-2.0

use super::*;
use crate::tokenizer;

// ---------------------------------------------------------------- resolution

#[test]
fn the_override_beats_the_root_beats_local_data() {
    let overridden = DictionaryPaths::decide(
        Some(PathBuf::from("override")),
        Some(Path::new("root")),
        Some(PathBuf::from("data")),
    )
    .unwrap();
    assert_eq!(overridden.root(), Path::new("override"));

    let rooted =
        DictionaryPaths::decide(None, Some(Path::new("root")), Some(PathBuf::from("data")))
            .unwrap();
    assert_eq!(rooted.root(), Path::new("root").join("dictionaries"));

    let defaulted = DictionaryPaths::decide(None, None, Some(PathBuf::from("data"))).unwrap();
    assert_eq!(
        defaulted.root(),
        Path::new("data").join("Koru").join("dictionaries")
    );

    assert_eq!(DictionaryPaths::decide(None, None, None), None);
}

/// A `--root` run still finds the machine's dictionaries, and installs there;
/// the rooted tree wins for a language it has.
#[test]
fn a_rooted_run_falls_back_to_the_machine_tree_per_language() {
    let paths = DictionaryPaths::decide(None, Some(Path::new("root")), Some(PathBuf::from("data")))
        .unwrap();
    let machine = Path::new("data").join("Koru").join("dictionaries");
    let rooted = Path::new("root").join("dictionaries");
    let on_disk = [machine.join("en-US"), machine.join("de"), rooted.join("de")];
    let exists = |file: &Path| {
        on_disk
            .iter()
            .any(|dir| file.parent() == Some(dir.as_path()))
    };
    assert_eq!(
        paths.pair_where(SpellLang::EnUs, exists).0,
        machine.join("en-US").join(AFF)
    );
    assert_eq!(
        paths.pair_where(SpellLang::De, exists).0,
        rooted.join("de").join(AFF)
    );
    assert_eq!(
        paths.pair_where(SpellLang::Fr, exists).0,
        rooted.join("fr").join(AFF),
        "a language nowhere reports the rooted tree"
    );
    assert_eq!(paths.install_root(), machine.as_path());
    // An override is exactly where it says, and installs there too.
    let overridden = DictionaryPaths::at("d");
    assert_eq!(overridden.install_root(), Path::new("d"));
}

#[test]
fn publication_exposes_one_complete_pair_and_ignores_staging() {
    let paths = DictionaryPaths::decide(None, Some(Path::new("root")), Some(PathBuf::from("data")))
        .unwrap();
    let legacy = paths.root().join("en-US");
    let installed = DictionaryPaths::download_dir(paths.install_root(), SpellLang::EnUs);
    let staging = paths.root().join("en-US").join(".download-another-process");
    let mut files = vec![legacy.join(AFF), staging.join(AFF), staging.join(DIC)];
    let resolve = |files: &Vec<PathBuf>| {
        paths.pair_where(SpellLang::EnUs, |file| files.iter().any(|p| p == file))
    };
    // Neither a half-written legacy pair nor a complete uncommitted download
    // makes the language installed. Publication is one directory operation.
    assert_eq!(resolve(&files), (legacy.join(AFF), legacy.join(DIC)));
    files.extend([installed.join(AFF), installed.join(DIC)]);
    assert_eq!(resolve(&files), (installed.join(AFF), installed.join(DIC)));
    // Explicit, complete overrides retain precedence; never mix their halves
    // with the machine download while only one override file exists.
    files.push(legacy.join(DIC));
    assert_eq!(resolve(&files), (legacy.join(AFF), legacy.join(DIC)));
}

#[test]
fn a_language_directory_is_its_tag() {
    let paths = DictionaryPaths::at("d");
    assert_eq!(
        paths.aff(SpellLang::EnUs),
        Path::new("d").join("en-US").join("dictionary.aff")
    );
    assert_eq!(
        paths.dic(SpellLang::PtBr),
        Path::new("d").join("pt-BR").join("dictionary.dic")
    );
}

// ---------------------------------------------------------------- fail open

/// A load that fails is remembered as `Failed` and not retried — the files
/// will not become valid by being re-read every debounce — and `reload` is
/// the one way out. The path names a directory that does not exist, so the
/// only disk activity is a failed metadata lookup.
#[test]
fn a_failed_load_is_cached_until_reload() {
    let manager = DictionaryManager::new(DictionaryPaths::at("__koru_spell_no_such_dir__"));
    assert_eq!(manager.state(SpellLang::De), DictionaryState::NotInstalled);
    assert!(manager.load(SpellLang::De).is_err());
    assert_eq!(manager.state(SpellLang::De), DictionaryState::Failed);
    let Err(second) = manager.load(SpellLang::De) else {
        panic!("the second load must fail from the cache")
    };
    assert!(
        second.message.contains("not retrying"),
        "the second load must answer from the cache, got: {}",
        second.message
    );
    assert!(manager.get(SpellLang::De).is_none());
    // `reload` forgets and tries again — still failing here, but freshly.
    let Err(third) = manager.reload(SpellLang::De) else {
        panic!("reload against a missing directory must fail")
    };
    assert!(!third.message.contains("not retrying"));
}

// ------------------------------------------------------------------- corpus

/// The corpus fixtures, compiled in. One entry per language; P6 fills the
/// other six. The harness is the acceptance gate: a language is enabled only
/// when its corpus passes against its real dictionary.
const FIXTURES: &[(SpellLang, &str)] = &[
    (SpellLang::EnUs, include_str!("../../fixtures/en-US.json")),
    (SpellLang::EnGb, include_str!("../../fixtures/en-GB.json")),
    (SpellLang::Es, include_str!("../../fixtures/es.json")),
    (SpellLang::Fr, include_str!("../../fixtures/fr.json")),
    (SpellLang::PtBr, include_str!("../../fixtures/pt-BR.json")),
    (SpellLang::De, include_str!("../../fixtures/de.json")),
    (SpellLang::Ar, include_str!("../../fixtures/ar.json")),
];

#[derive(serde::Deserialize)]
struct Fixture {
    cases: Vec<Case>,
}

#[derive(serde::Deserialize)]
struct Case {
    name: String,
    text: String,
    expected: Vec<Expected>,
}

#[derive(serde::Deserialize)]
struct Expected {
    word: String,
    /// Byte offsets over the case's text. May be omitted when the word occurs
    /// exactly once — the runner locates it, which keeps multi-byte fixtures
    /// (accents, Arabic) authorable without counting bytes by hand.
    #[serde(default)]
    start: Option<usize>,
    #[serde(default)]
    end: Option<usize>,
}

impl Expected {
    fn resolved(&self, text: &str) -> (String, usize, usize) {
        match (self.start, self.end) {
            (Some(start), Some(end)) => (self.word.clone(), start, end),
            _ => {
                let start = text.find(&self.word).unwrap_or_else(|| {
                    panic!("expected word {:?} not found in {text:?}", self.word)
                });
                assert!(
                    text[start + self.word.len()..].find(&self.word).is_none(),
                    "expected word {:?} occurs twice in {text:?}; give explicit offsets",
                    self.word
                );
                (self.word.clone(), start, start + self.word.len())
            }
        }
    }
}

/// The dictionaries the corpus runs against, or `None` and the reason to
/// skip. **Only the explicit `KORU_DICTIONARIES` is honoured** (plan §P1,
/// rev1 F6): resolving the default directory here would make a dev machine
/// with dictionaries installed run a different suite than CI.
fn corpus_dictionaries() -> Option<DictionaryPaths> {
    koru_env::value(koru_env::name::KORU_DICTIONARIES).map(DictionaryPaths::at)
}

#[test]
fn the_corpus_passes_against_the_real_dictionaries() {
    let Some(paths) = corpus_dictionaries() else {
        eprintln!(
            "KORU_DICTIONARIES is not set; corpus skipped (run scripts/setup/fetch-dictionaries and point it there)"
        );
        return;
    };
    let manager = DictionaryManager::new(paths);
    for (lang, fixture) in FIXTURES {
        let provider = match manager.load(*lang) {
            Ok(provider) => provider,
            Err(error) => panic!(
                "{} dictionary failed to load: {}",
                lang.tag(),
                error.message
            ),
        };
        let fixture: Fixture = serde_json::from_str(fixture).expect("fixture parses");
        for case in fixture.cases {
            let found: Vec<(String, usize, usize)> = tokenizer::words(&case.text, &[])
                .into_iter()
                .filter(|token| !provider.check(&token.check))
                .map(|token| (token.text.to_string(), token.range.start, token.range.end))
                .collect();
            let expected: Vec<(String, usize, usize)> = case
                .expected
                .iter()
                .map(|e| e.resolved(&case.text))
                .collect();
            assert_eq!(
                found,
                expected,
                "{}: case '{}' on {:?}",
                lang.tag(),
                case.name,
                case.text
            );
        }
    }
}

// -------------------------------------------------------------------- bench

/// Numbers for the phase notes, not assertions (measure ratios, not clocks —
/// PRINCIPLES.md). Ignored so `cargo test` never depends on the machine;
/// run with:
/// `KORU_DICTIONARIES=<dir> cargo test -p koru-spell -- --ignored --nocapture bench`
/// Init cost per language, for the P6 compatibility report the delivery
/// decision reads. Ignored for the same reason the en-US bench is.
#[test]
#[ignore = "prints per-language init timings; needs KORU_DICTIONARIES"]
fn bench_all_languages() {
    let Some(paths) = corpus_dictionaries() else {
        eprintln!("KORU_DICTIONARIES is not set; bench skipped");
        return;
    };
    for lang in SpellLang::ALL {
        let manager = DictionaryManager::new(paths.clone());
        let started = std::time::Instant::now();
        match manager.load(lang) {
            Ok(_) => eprintln!("{}: init {:?}", lang.tag(), started.elapsed()),
            Err(error) => eprintln!("{}: failed: {}", lang.tag(), error.message),
        }
    }
}

#[test]
#[ignore = "prints engine timings; needs KORU_DICTIONARIES"]
fn bench_en_us() {
    let Some(paths) = corpus_dictionaries() else {
        eprintln!("KORU_DICTIONARIES is not set; bench skipped");
        return;
    };
    let manager = DictionaryManager::new(paths);
    let started = std::time::Instant::now();
    let provider = manager.load(SpellLang::EnUs).expect("en-US loads");
    let init = started.elapsed();

    let words = [
        "the",
        "quick",
        "brown",
        "fox",
        "jumps",
        "over",
        "lazy",
        "dogs",
        "paragraph",
        "immediately",
        "necessary",
        "occasionally",
        "misspelled",
        "teh",
        "recieve",
        "wierd",
    ];
    let rounds = 10_000;
    let started = std::time::Instant::now();
    let mut valid = 0usize;
    for _ in 0..rounds {
        for word in words {
            valid += provider.check(word) as usize;
        }
    }
    let checking = started.elapsed();
    let checks = rounds * words.len();

    let started = std::time::Instant::now();
    let mut suggestions = 0usize;
    for word in ["teh", "recieve", "wierd", "runing", "dogz"] {
        suggestions += provider.suggest(word, 5).len();
    }
    let suggesting = started.elapsed();

    eprintln!("en-US init: {init:?}");
    eprintln!(
        "checks: {checks} in {checking:?} ({:.0}/ms), {valid} valid",
        checks as f64 / checking.as_secs_f64() / 1000.0
    );
    eprintln!("5 suggest calls: {suggesting:?} ({suggestions} suggestions)");
}
