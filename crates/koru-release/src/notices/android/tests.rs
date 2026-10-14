// SPDX-License-Identifier: MPL-2.0

use super::*;

fn root() -> &'static Path {
    Path::new(env!("CARGO_MANIFEST_DIR"))
        .parent()
        .and_then(Path::parent)
        .expect("workspace root")
}

fn pom(name: &str, url: &str) -> PomLicense {
    PomLicense {
        name: Some(name.to_owned()),
        url: Some(url.to_owned()),
    }
}

fn library(coordinates: &str, licenses: Vec<PomLicense>, files: Vec<File>) -> Library {
    Library {
        coordinates: coordinates.to_owned(),
        url: None,
        licenses,
        files,
    }
}

fn file(name: &str, text: &str) -> File {
    File {
        name: name.to_owned(),
        text: text.to_owned(),
    }
}

/// The spellings the app's POMs use today, by name or by URL.
#[test]
fn pom_licenses_map_to_spdx() {
    for (name, url, id) in [
        (
            "The Apache Software License, Version 2.0",
            "http://www.apache.org/licenses/LICENSE-2.0.txt",
            "Apache-2.0",
        ),
        ("Apache-2.0", "", "Apache-2.0"),
        (
            "",
            "https://www.apache.org/licenses/LICENSE-2.0",
            "Apache-2.0",
        ),
        (
            "Android Software Development Kit License",
            "https://developer.android.com/studio/terms.html",
            ANDROID_SDK,
        ),
        (
            "BSD-3-Clause",
            "https://opensource.org/licenses/BSD-3-Clause",
            "BSD-3-Clause",
        ),
        ("MIT", "", "MIT"),
        (
            "ML Kit Terms of Service",
            "https://developers.google.com/ml-kit/terms",
            ML_KIT,
        ),
    ] {
        assert_eq!(license_id(&pom(name, url)), Some(id), "{name} {url}");
    }
    assert_eq!(license_id(&pom("Eclipse Public License 1.0", "")), None);
}

/// **An Apache-2.0 library without files gets the standard text**, which is
/// all that license asks; Google's proprietary library gets its statement and
/// keeps the third-party texts it carries.
#[test]
fn libraries_get_their_texts() {
    let apache = pom("The Apache Software License, Version 2.0", "");
    let google = pom("Android Software Development Kit License", "");
    let notices = notices(
        root(),
        &[
            library("androidx.core:core:1.16.0", vec![apache.clone()], vec![]),
            library(
                "com.google.android.gms:play-services-base:18.5.0",
                vec![google],
                vec![
                    file(
                        "third_party_licenses.json",
                        r#"{"Thing": {"start": 8, "length": 4}}"#,
                    ),
                    file("third_party_licenses.txt", "Thing:\n\nText"),
                ],
            ),
            library(
                "com.example:carries:1.0",
                vec![apache],
                vec![file("META-INF/LICENSE", "Its own text")],
            ),
        ],
    )
    .unwrap();
    let names: Vec<&str> = notices.iter().map(|n| n.name.as_str()).collect();
    assert_eq!(
        names,
        [
            "androidx.core:core",
            "com.example:carries",
            "com.google.android.gms:play-services-base"
        ]
    );
    assert_eq!(notices[0].version, "1.16.0");
    assert!(
        notices[0].texts[0]
            .0
            .starts_with("Apache-2.0 — standard text")
    );
    assert!(notices[0].texts[0].1.contains("Apache License"));
    assert_eq!(
        notices[1].texts,
        [("META-INF/LICENSE".to_owned(), "Its own text".to_owned())]
    );
    let google = &notices[2];
    assert_eq!(google.license, ANDROID_SDK);
    assert_eq!(google.texts[0].0, ANDROID_SDK);
    assert_eq!(
        google.texts[1],
        ("third_party_licenses: Thing".to_owned(), "Text".to_owned())
    );
}

/// **A library with no file uses its recorded upstream text**: protobuf's
/// jars carry none and its BSD-3-Clause needs Google's copyright line.
#[test]
fn a_recorded_library_gets_upstreams_text() {
    let notices = notices(
        root(),
        &[library(
            "com.google.protobuf:protobuf-javalite:4.35.1",
            vec![pom("BSD-3-Clause", "")],
            vec![],
        )],
    )
    .unwrap();
    let (label, text) = &notices[0].texts[0];
    assert!(
        label
            .starts_with("LICENSE — from upstream at https://github.com/protocolbuffers/protobuf/"),
        "{label}"
    );
    assert!(text.starts_with("Copyright 2008 Google Inc."));
    assert!(read_maven_records(&root().join(MAVEN_DIR)).is_ok());
}

/// **A library whose license needs a copyright line, and that carries no
/// file, is refused**: there is nobody to name. So is one whose POM names a
/// license this module does not know.
#[test]
fn a_library_without_a_complete_text_is_refused() {
    for licenses in [
        vec![pom("MIT", "")],
        vec![pom("Eclipse Public License 1.0", "")],
        vec![],
    ] {
        let error = notices(root(), &[library("g:a:1", licenses, vec![])]).unwrap_err();
        assert!(error.contains("g:a:1"), "{error}");
    }
    // An unknown license is refused even when the artifact carries a text.
    let carried = library(
        "g:a:1",
        vec![pom("Some Vendor Terms", "")],
        vec![file("LICENSE", "terms")],
    );
    assert!(notices(root(), &[carried]).is_err());
}

/// **Spans are bytes, and they are checked against the headings.** A text
/// with a non-ASCII character before a span still splits where Google's index
/// says, and a span that does not follow its heading is an error.
#[test]
fn third_party_licenses_split_on_their_headings() {
    let text = "\n\nÄ:\n\nfirst\n\nB:\n\nsecond";
    let a_start = text.find("first").unwrap();
    let b_start = text.find("second").unwrap();
    let index = format!(
        r#"{{"Ä": {{"start": {a_start}, "length": 5}}, "B": {{"start": {b_start}, "length": 6}}}}"#
    );
    assert_eq!(
        split_third_party(&index, text).unwrap(),
        [
            ("third_party_licenses: B".to_owned(), "second".to_owned()),
            ("third_party_licenses: Ä".to_owned(), "first".to_owned()),
        ]
    );
    // Some libraries put the text right after the heading (Dagger, in
    // transport-runtime).
    assert_eq!(
        split_third_party(r#"{"Dagger": {"start": 7, "length": 4}}"#, "Dagger:text").unwrap()[0].1,
        "text"
    );
    // Character offsets land a byte early after the "Ä" and miss the heading.
    let tight = "\n\nÄ:\n\nfirst\n\nB:second";
    let char_start = tight.find("second").unwrap() - 1;
    let index = format!(r#"{{"B": {{"start": {char_start}, "length": 6}}}}"#);
    assert!(split_third_party(&index, tight).is_err());
    assert!(split_third_party(r#"{"B": {"start": 0, "length": 3}}"#, text).is_err());
    assert!(split_third_party(r#"{"B": {"start": 900, "length": 3}}"#, text).is_err());
}
