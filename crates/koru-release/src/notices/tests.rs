// SPDX-License-Identifier: MPL-2.0

use super::*;

fn package(id: &str, name: &str, license: &str) -> Package {
    Package {
        id: id.to_owned(),
        name: name.to_owned(),
        version: "1.0.0".to_owned(),
        license: Some(license.to_owned()),
        license_file: None,
        authors: Vec::new(),
        repository: None,
        source: Some("registry+https://github.com/rust-lang/crates.io-index".to_owned()),
        manifest_path: PathBuf::from(format!("/registry/{name}/Cargo.toml")),
        targets: vec![Target {
            kind: vec!["lib".to_owned()],
        }],
    }
}

fn dep(pkg: &str, kind: Option<&str>) -> NodeDep {
    NodeDep {
        pkg: pkg.to_owned(),
        dep_kinds: vec![DepKind {
            kind: kind.map(str::to_owned),
        }],
    }
}

/// `app` (ours) → `serde` (normal) → `serde_derive` (proc-macro);
/// `app` → `cc` (build only); `app` → `tempfile` (dev only);
/// `app` → `koru-core` (ours, normal) → `smallvec` (normal).
fn graph() -> Metadata {
    let mut derive = package("serde_derive", "serde_derive", "MIT OR Apache-2.0");
    derive.targets = vec![Target {
        kind: vec!["proc-macro".to_owned()],
    }];
    let mut app = package("app", "app", "MPL-2.0");
    app.source = None;
    let mut core = package("core", "koru-core", "MPL-2.0");
    core.source = None;
    Metadata {
        packages: vec![
            app,
            core,
            package("serde", "serde", "MIT OR Apache-2.0"),
            derive,
            package("cc", "cc", "MIT OR Apache-2.0"),
            package("tempfile", "tempfile", "MIT OR Apache-2.0"),
            package("smallvec", "smallvec", "MIT OR Apache-2.0"),
        ],
        workspace_members: vec!["app".to_owned(), "core".to_owned()],
        resolve: Resolve {
            nodes: vec![
                Node {
                    id: "app".to_owned(),
                    deps: vec![
                        dep("serde", None),
                        dep("cc", Some("build")),
                        dep("tempfile", Some("dev")),
                        dep("core", None),
                    ],
                },
                Node {
                    id: "serde".to_owned(),
                    deps: vec![dep("serde_derive", None)],
                },
                Node {
                    id: "core".to_owned(),
                    deps: vec![dep("smallvec", None)],
                },
            ],
        },
    }
}

/// Only what can be inside a binary is listed: normal dependencies, through
/// our own crates, minus proc-macros, build and dev dependencies, and minus
/// our own packages.
#[test]
fn only_code_that_ships_is_listed() {
    let metadata = graph();
    let names: Vec<_> = shipped(&metadata, &["app"])
        .unwrap()
        .into_iter()
        .map(|package| package.name.as_str())
        .collect();
    assert_eq!(names, ["serde", "smallvec"]);
}

/// **A Koru Cloud package in a shipped binary's graph stops the release.**
/// The open-source app must never link one, and this is the last place that
/// can notice before the package exists.
#[test]
fn a_proprietary_package_in_the_graph_is_refused() {
    let mut metadata = graph();
    let core = metadata
        .packages
        .iter_mut()
        .find(|package| package.id == "core")
        .unwrap();
    core.license = Some(PROPRIETARY.to_owned());
    let error = shipped(&metadata, &["app"]).unwrap_err();
    assert!(error.contains("koru-core"), "{error}");
}

/// A proprietary package that is only a dev or build dependency puts no code
/// in the binary, so the shipped walk does not see it, and it ships nothing
/// that needs a notice.
#[test]
fn a_proprietary_dev_dependency_is_not_in_the_shipped_graph() {
    let mut metadata = graph();
    let tempfile = metadata
        .packages
        .iter_mut()
        .find(|package| package.id == "tempfile")
        .unwrap();
    tempfile.license = Some(PROPRIETARY.to_owned());
    tempfile.source = None;
    metadata.workspace_members.push("tempfile".to_owned());
    assert!(shipped(&metadata, &["app"]).is_ok());
}

#[test]
fn an_unknown_root_is_an_error() {
    assert!(shipped(&graph(), &["koru-nothing"]).is_err());
}

#[test]
fn the_permissive_option_is_chosen_and_copyleft_only_is_not() {
    assert_eq!(choose("MIT OR Apache-2.0"), Some("MIT"));
    assert_eq!(choose("Apache-2.0/MIT"), Some("MIT"));
    assert_eq!(
        choose("MIT OR Apache-2.0 OR LGPL-2.1-or-later"),
        Some("MIT")
    );
    assert_eq!(choose("Zlib OR Apache-2.0 OR MIT"), Some("MIT"));
    // A conjunction needs every text and an exception needs its own; one
    // standard license would be an incomplete notice, so neither is guessed.
    assert_eq!(choose("(MIT OR Apache-2.0) AND Unicode-3.0"), None);
    assert_eq!(choose("Apache-2.0 WITH LLVM-exception"), None);
    assert_eq!(choose("MIT AND BSD-3-Clause"), None);
    // Parentheses delimit operators; a conjunction beside one is still one.
    assert_eq!(choose("MIT AND(BSD-3-Clause)"), None);
    assert_eq!(choose("(MIT)AND(BSD-3-Clause)"), None);
    assert_eq!(choose("(Apache-2.0)WITH(LLVM-exception)"), None);
    assert_eq!(choose("MIT and BSD-3-Clause"), None);
    // A parenthesized choice is still a choice.
    assert_eq!(choose("(MIT)OR(Apache-2.0)"), Some("MIT"));
    assert_eq!(choose("MPL-2.0+"), Some("MPL-2.0"));
    assert_eq!(choose("GPL-3.0-only"), None);
    assert_eq!(choose(""), None);
}

/// Every license `choose` can return has a standard text to reproduce.
#[test]
fn every_preferred_license_has_a_standard_text() {
    let root = Path::new(env!("CARGO_MANIFEST_DIR"))
        .parent()
        .and_then(Path::parent)
        .expect("workspace root");
    for id in PREFERENCE {
        assert!(
            root.join(SPDX_DIR).join(format!("{id}.txt")).is_file(),
            "{SPDX_DIR}/{id}.txt"
        );
    }
}

#[test]
fn the_copyright_line_names_the_authors_without_their_email() {
    let mit = "MIT License\n\nCopyright (c) <year> <copyright holders>\n";
    assert_eq!(
        fill_copyright(
            mit,
            "x",
            &[
                "Ada Lovelace <ada@example.com>".to_owned(),
                "Bob".to_owned()
            ]
        ),
        "MIT License\n\nCopyright (c) Ada Lovelace, Bob\n"
    );
    assert_eq!(
        fill_copyright(mit, "x", &[]),
        "MIT License\n\nCopyright (c) the x authors\n"
    );
}

#[test]
fn license_files_are_recognized_by_name() {
    for name in [
        "LICENSE",
        "LICENSE-MIT",
        "license.txt",
        "LICENCE",
        "COPYING",
        "UNLICENSE",
        "NOTICE",
        "COPYRIGHT",
    ] {
        assert!(is_license_file(name), "{name}");
    }
    for name in ["Cargo.toml", "README.md", "src", "build.rs"] {
        assert!(!is_license_file(name), "{name}");
    }
}

/// A thousand copies of Apache-2.0 are printed once, under every crate that
/// carries it; a text that differs (an MIT with its own copyright line) is not
/// merged into another.
#[test]
fn identical_texts_are_printed_once() {
    let apache = "Apache License\nVersion 2.0".to_owned();
    let notice = |name: &str, text: &str| CrateNotice {
        name: name.to_owned(),
        version: "1.0.0".to_owned(),
        license: "Apache-2.0".to_owned(),
        location: format!("https://crates.io/crates/{name}/1.0.0"),
        texts: vec![("LICENSE".to_owned(), text.to_owned())],
        inferred: false,
    };
    let out = render(
        &[
            notice("a", &apache),
            notice("b", "Apache License\r\nVersion  2.0"),
            notice("c", "MIT License\nCopyright (c) C"),
        ],
        &[],
        &[],
        None,
    );
    assert_eq!(out.matches("Apache License").count(), 1, "{out}");
    assert!(out.contains("Used by: a 1.0.0 (LICENSE), b 1.0.0 (LICENSE)"));
    assert!(out.contains("Used by: c 1.0.0 (LICENSE)"));
    assert!(out.contains("a 1.0.0 — Apache-2.0 — https://crates.io/crates/a/1.0.0"));
}

#[test]
fn a_bundled_file_needs_its_header() {
    let good = "Component: X\nSource: https://example.com\nFiles: koru\nLicense: MIT\n---\ntext\n";
    let parsed = parse_bundled(good).unwrap();
    assert_eq!(parsed.field("License"), Some("MIT"));
    assert_eq!(parsed.text, "text\n");
    assert!(
        parse_bundled("Component: X\n---\ntext").is_err(),
        "missing fields"
    );
    assert!(parse_bundled("no header at all").is_err());
}

fn record(disposition: &str, license: &str) -> Bundled {
    parse_record(
        &format!(
            "Crate: x\nChecked: 1.0.0\nLicense: {license}\nDisposition: {disposition}\n\
             File: LICENSE-MIT\nSource: https://example.com/x/blob/abc/LICENSE-MIT\n---\nMIT text\n"
        ),
        RECORD_KEYS,
    )
    .unwrap()
}

/// **A record says where its text came from**, in the label the file prints:
/// upstream's file at a revision, or our judgment where upstream has none.
#[test]
fn a_recorded_text_names_its_origin() {
    let crate_ = package("x", "x", "MIT OR Apache-2.0");
    let (label, text) =
        recorded_text(&crate_, "MIT OR Apache-2.0", &record("upstream", "MIT")).unwrap();
    assert_eq!(
        label,
        "LICENSE-MIT — from upstream at https://example.com/x/blob/abc/LICENSE-MIT (checked at 1.0.0)"
    );
    assert_eq!(text, "MIT text\n");
    let (label, _) =
        recorded_text(&crate_, "MIT OR Apache-2.0", &record("judgment", "MIT")).unwrap();
    assert!(
        label.starts_with("MIT — standard text; upstream has no license file"),
        "{label}"
    );
    assert!(!label.contains("INFERRED"));
    assert!(recorded_text(&crate_, "MIT", &record("guessed", "MIT")).is_err());
}

/// **A record for a license the crate does not declare is refused**, so a
/// crate that relicenses is not attributed under its old license.
#[test]
fn a_record_must_name_a_declared_license() {
    let crate_ = package("x", "x", "Apache-2.0");
    let error = recorded_text(&crate_, "Apache-2.0", &record("upstream", "MIT")).unwrap_err();
    assert!(error.contains("records MIT"), "{error}");
    // `MIT-0` is not `MIT`: tokens are compared whole.
    assert!(recorded_text(&crate_, "MIT-0", &record("upstream", "MIT")).is_err());
}

/// **Every record in the tree parses and is filed under its crate's name.**
/// The generator reads them by name, so a record filed under another name
/// would never be used, and its crate would be refused at release.
#[test]
fn every_crate_record_parses() {
    let root = Path::new(env!("CARGO_MANIFEST_DIR"))
        .parent()
        .and_then(Path::parent)
        .expect("workspace root");
    let records = read_records(&root.join(CRATES_DIR)).expect("every record parses");
    assert!(records.len() >= 38, "{}", records.len());
    for (name, record) in &records {
        let disposition = record.field("Disposition").unwrap();
        assert!(
            ["upstream", "judgment"].contains(&disposition),
            "{name}: {disposition}"
        );
        match disposition {
            "upstream" => assert!(record.field("File").is_some(), "{name} names no File"),
            _ => assert!(
                record
                    .field("Issue")
                    .is_some_and(|issue| issue.starts_with("licensing/UPSTREAM-ISSUES.md")),
                "{name}: a judgment needs a drafted upstream issue"
            ),
        }
        assert!(!record.text.trim().is_empty(), "{name} has no text");
        assert!(
            !record.text.contains("<copyright holders>"),
            "{name}'s text still has the placeholder"
        );
    }
}

/// A component names the platforms it is linked on; without the line it is
/// linked everywhere.
#[test]
fn a_component_can_be_limited_to_platforms() {
    let everywhere = parse_bundled(
        "Component: X
Source: s
Files: f
License: MIT
---
t",
    )
    .unwrap();
    assert!(everywhere.applies_to("windows") && everywhere.applies_to("macos"));
    let mac = parse_bundled(
        "Component: X
Source: s
Files: f
License: MIT
Platforms: macos
---
t",
    )
    .unwrap();
    assert!(mac.applies_to("macos"));
    assert!(!mac.applies_to("windows"));
}

/// **No text-to-speech library ships, on any platform.** Every target links
/// upstream's TTS-free sherpa-onnx archive, which has no eSpeak NG, ucd or
/// piper-phonemize. A notice for one of them would claim GPL-3.0 code in the
/// binaries; if one is ever needed again, that build has linked eSpeak NG and
/// is a GPL work (third-party/patches/sherpa-onnx.md).
#[test]
fn no_text_to_speech_library_is_attributed_on_any_platform() {
    for component in bundled() {
        let names = [
            component.field("Component").unwrap_or_default(),
            component.field("Source").unwrap_or_default(),
        ]
        .join(" ")
        .to_ascii_lowercase();
        for tts in ["espeak", "piper", "ucd-tools"] {
            assert!(
                !names.contains(tts),
                "{names} is attributed, and Koru links no text-to-speech"
            );
        }
    }
}

fn bundled() -> Vec<Bundled> {
    let root = Path::new(env!("CARGO_MANIFEST_DIR"))
        .parent()
        .and_then(Path::parent)
        .expect("workspace root");
    read_bundled(&root.join(BUNDLED_DIR)).expect("every bundled license file parses")
}

/// **Every font the package ships is attributed.** `FONTS` is the list the
/// package copies; a face added there without a license file here would ship
/// without the OFL notice it requires.
#[test]
fn every_shipped_font_is_attributed() {
    let bundled = bundled();
    for font in crate::package::FONTS {
        assert!(
            bundled.iter().any(|component| component
                .field("Files")
                .is_some_and(|files| files.split(", ").any(|file| file == *font))),
            "{font} is shipped but no file in {BUNDLED_DIR} names it"
        );
    }
}

/// **Every face the Android app bundles is attributed, for Android only.** The
/// `android/` entries in `scripts/setup/fonts.manifest` are what `fetch-fonts`
/// places in `assets/fonts/android/`, the directory Gradle packages. A face added there without a
/// license file here would ship without its OFL notice, and a notice without
/// `Platforms: android` would list an Android-only face in every desktop
/// package's notices.
#[test]
fn every_android_font_is_attributed_for_android_only() {
    let bundled = bundled();
    let faces: Vec<&str> = include_str!("../../../../scripts/setup/fonts.manifest")
        .lines()
        .filter_map(|line| line.split_once('=')?.0.strip_prefix("android/"))
        .collect();
    assert!(
        !faces.is_empty(),
        "scripts/setup/fonts.manifest pins no Android face"
    );
    for face in faces {
        let component = bundled
            .iter()
            .find(|component| {
                component
                    .field("Files")
                    .is_some_and(|files| files.split(", ").any(|file| file == face))
            })
            .unwrap_or_else(|| {
                panic!("{face} is bundled on Android but no file in {BUNDLED_DIR} names it")
            });
        assert!(
            component.applies_to("android"),
            "{face}'s notice omits Android"
        );
        for desktop in ["windows", "macos", "linux"] {
            assert!(
                !component.applies_to(desktop),
                "{face}'s notice applies to {desktop}"
            );
        }
    }
}

/// **Every bundled component says where it is linked**, and nothing only the
/// desktop links is attributed on Android. The Android library links neither
/// sherpa-onnx nor whisper.cpp nor koru-blocks, and the APK carries its own
/// static faces, so the desktop's fonts and native libraries in its notices
/// would claim components it does not contain.
#[test]
fn every_bundled_component_names_its_platforms() {
    for component in bundled() {
        let name = component.field("Component").unwrap().to_owned();
        assert!(
            component.field("Platforms").is_some(),
            "{name} names no Platforms"
        );
        let fonts = component.field("Files").is_some_and(|files| {
            files
                .split(", ")
                .any(|file| crate::package::FONTS.contains(&file))
        });
        if fonts
            || component
                .field("Files")
                .is_some_and(|files| files.contains("koru-"))
        {
            assert!(
                !component.applies_to("android"),
                "{name} is attributed on Android"
            );
        }
    }
}

/// The native libraries `sherpa-onnx-sys` links statically are all attributed:
/// the list is `SHERPA_ONNX_STATIC_LIBS` in its build script, mapped to the
/// projects they come from.
#[test]
fn every_statically_linked_voice_library_is_attributed() {
    let components: Vec<String> = bundled()
        .iter()
        .filter_map(|component| component.field("Component").map(str::to_owned))
        .collect();
    for project in [
        "sherpa-onnx",
        "ONNX Runtime",
        "kaldi-native-fbank",
        "KISS FFT",
        "kaldi-decoder",
        "OpenFst",
        "simple-sentencepiece",
        "whisper.cpp and ggml",
    ] {
        assert!(components.iter().any(|name| name == project), "{project}");
    }
}

/// **The real file, for this tree.** Runs `cargo metadata` and reads every
/// crate's license files, so it is ignored by default; run it by hand to see
/// what a package would carry:
///
/// `cargo test -p koru-release generates_the_notices_for_this_tree -- --ignored`
///
/// The file lands in the target directory as `THIRD-PARTY-NOTICES.txt`.
#[test]
#[ignore = "runs cargo metadata and reads the registry; run by hand"]
fn generates_the_notices_for_this_tree() {
    let root = Path::new(env!("CARGO_MANIFEST_DIR"))
        .parent()
        .and_then(Path::parent)
        .expect("workspace root");
    let target = std::env::var_os("CARGO_TARGET_DIR")
        .map(PathBuf::from)
        .unwrap_or_else(|| root.join("target"));
    let destination = target.join(FILE);
    let packages = crate::package::CHANNEL_BINARIES
        .iter()
        .map(|(package, _)| *package)
        .chain(["koru-updater"])
        .collect::<Vec<_>>();
    write(root, &packages, &destination).expect("the notices generate");
    let text = fs::read_to_string(&destination).unwrap();
    for expected in ["Part 1", "Part 2", "OFL", "loro", "serde"] {
        assert!(text.contains(expected), "{expected}");
    }
    // No platform links eSpeak NG, so none attributes it.
    assert!(!text.contains("eSpeak NG"), "eSpeak NG is attributed");
    // Every crate without a license file has a record, or `write` refused.
    assert!(!text.contains("INFERRED"));
    println!("{} ({} bytes)", destination.display(), text.len());
}

/// **The Android crates, for this tree**: the native library's graph for
/// both ABIs, refused the same way a release is if a crate has no record.
/// Ignored for the same reason; Gradle runs the real thing with the Maven
/// libraries added.
///
/// `cargo test -p koru-release generates_the_android_notices_for_this_tree -- --ignored`
#[test]
#[ignore = "runs cargo metadata and reads the registry; run by hand"]
fn generates_the_android_notices_for_this_tree() {
    let root = Path::new(env!("CARGO_MANIFEST_DIR"))
        .parent()
        .and_then(Path::parent)
        .expect("workspace root");
    let build = Build {
        packages: android::PACKAGES,
        triples: android::TRIPLES.iter().map(|t| (*t).to_owned()).collect(),
        os: "android",
        unrecorded: Unrecorded::Refuse,
        source: Some("https://github.com/koruworks/mono/tree/0000000"),
    };
    let text = generate(root, &build, &[]).expect("the Android notices generate");
    for expected in ["Inter-Regular.ttf", "Manrope-SemiBold.ttf", "jni", "loro"] {
        assert!(text.contains(expected), "{expected}");
    }
    for absent in ["sherpa-onnx", "whisper.cpp", "JetBrains Mono", "INFERRED"] {
        assert!(!text.contains(absent), "{absent}");
    }
    assert!(
        text.contains(
            "The source code of this build: https://github.com/koruworks/mono/tree/0000000"
        )
    );
}
