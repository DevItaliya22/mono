// SPDX-License-Identifier: MPL-2.0

use super::*;

/// The seal on D1: `spellbook` may be named by exactly one module. The sources
/// are embedded at compile time, so this walks no disk — it is a tripwire
/// that fires the moment an engine type or call leaks toward the public
/// surface, which is the moment the engine stops being replaceable.
#[test]
fn the_engine_is_confined_to_engine_rs() {
    let sealed = [
        ("lib.rs", include_str!("lib.rs")),
        ("lang.rs", include_str!("lang.rs")),
        ("tokenizer.rs", include_str!("tokenizer.rs")),
        ("dictionaries.rs", include_str!("dictionaries.rs")),
        ("tests.rs", include_str!("tests.rs")),
    ];
    // Assembled at runtime so this file's own source does not trip the wire.
    let needle: String = ["spell", "book", "::"].concat();
    for (name, source) in sealed {
        assert!(
            !source.contains(&needle),
            "{name} reaches into the engine; only engine.rs may"
        );
    }
}

/// **No dictionary is ever compiled into a binary** (`licensing/THIRD-PARTY.md`,
/// ⚑2). A GPL dictionary beside the app is mere aggregation; the same bytes
/// inside an executable make it a combined GPL work — the eSpeak NG problem
/// again. So this walks every Rust source the workspace builds and fails on an
/// `include_bytes!`/`include_str!` of a Hunspell `.aff` or `.dic`, on a build
/// script that reaches for dictionaries, and on a dictionary file sitting in a
/// source directory, where one line of code would embed or bundle it.
///
/// Tests embed none either: the one engine test here parses a three-word
/// inline dictionary, and the corpus fixtures are JSON cases, not word lists.
/// A synthetic fixture file, if one is ever needed, is listed in
/// `SYNTHETIC_FIXTURES` with what makes it ours.
#[test]
fn no_source_compiles_in_a_dictionary() {
    /// Workspace-relative paths of `.aff`/`.dic` files written by hand for a
    /// test, with nothing upstream in them. Empty: none exists.
    const SYNTHETIC_FIXTURES: &[&str] = &[];
    let workspace = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
        .parent()
        .and_then(std::path::Path::parent)
        .expect("crates/koru-spell sits two levels below the workspace");
    // Assembled at runtime so this file's own source does not trip the wire.
    let embeds = [
        ["include", "_bytes!"].concat(),
        ["include", "_str!"].concat(),
    ];
    let extensions = [[".", "aff"].concat(), [".", "dic"].concat()];
    let mut found = Vec::new();
    // Where workspace code lives; `third-party/` holds the fetched, patched
    // crates the binaries also link.
    for top in [
        "crates",
        "apps",
        "third-party",
        "assets",
        "contracts",
        "tests",
    ] {
        walk(&workspace.join(top), &mut |path| {
            let shown = path
                .strip_prefix(workspace)
                .unwrap_or(path)
                .to_string_lossy()
                .replace('\\', "/");
            let name = path
                .file_name()
                .and_then(|n| n.to_str())
                .unwrap_or_default()
                .to_ascii_lowercase();
            if extensions.iter().any(|ext| name.ends_with(ext.as_str())) {
                if !SYNTHETIC_FIXTURES.contains(&shown.as_str()) {
                    found.push(format!("{shown}: a dictionary file in a source directory"));
                }
                return;
            }
            if !name.ends_with(".rs") {
                return;
            }
            let Ok(source) = std::fs::read_to_string(path) else {
                return;
            };
            for embed in &embeds {
                for (at, _) in source.match_indices(embed.as_str()) {
                    // The macro's argument, up to its closing parenthesis.
                    let call = &source[at..];
                    let call = &call[..call.find(')').map_or(call.len(), |end| end + 1)];
                    let lower = call.to_ascii_lowercase();
                    if extensions
                        .iter()
                        .any(|ext| lower.contains(&format!("{ext}\"")))
                    {
                        found.push(format!("{shown}: {call}"));
                    }
                }
            }
            let dictionaries = ["KORU_", "DICTIONARIES"].concat();
            if name == "build.rs"
                && (source.contains(&dictionaries)
                    || extensions
                        .iter()
                        .any(|ext| source.contains(&format!("{ext}\""))))
            {
                found.push(format!("{shown}: a build script that reads dictionaries"));
            }
        });
    }
    assert!(
        found.is_empty(),
        "dictionaries must ship as separate files beside the app, never compiled in:\n{}",
        found.join("\n")
    );
}

/// Every file under `dir`, skipping build output and hidden directories.
fn walk(dir: &std::path::Path, visit: &mut dyn FnMut(&std::path::Path)) {
    let Ok(entries) = std::fs::read_dir(dir) else {
        return;
    };
    for entry in entries.flatten() {
        let path = entry.path();
        let Ok(kind) = entry.file_type() else {
            continue;
        };
        if kind.is_dir() {
            let name = entry.file_name();
            let name = name.to_string_lossy();
            if name.starts_with('.') || matches!(name.as_ref(), "target" | "build" | "node_modules")
            {
                continue;
            }
            walk(&path, visit);
        } else if kind.is_file() {
            visit(&path);
        }
    }
}

/// The public API exercised end to end with only Koru types in scope — what
/// "the editor never learns the engine exists" looks like as a call sequence.
/// The tiny inline dictionary keeps this off the disk.
#[test]
fn the_public_api_speaks_only_koru_types() {
    let provider =
        crate::engine::SpellbookDictionary::parse("SET UTF-8\n", "3\nhello\nworld\ntests\n")
            .expect("the inline dictionary parses");
    let provider: std::sync::Arc<dyn SpellProvider> = std::sync::Arc::new(provider);
    assert!(provider.check("hello"));
    assert!(!provider.check("helo"));
    let suggestions = provider.suggest("helo", 5);
    assert!(
        suggestions.iter().any(|s| s == "hello"),
        "suggestions for 'helo' should contain 'hello', got {suggestions:?}"
    );
    assert!(suggestions.len() <= 5);
}
