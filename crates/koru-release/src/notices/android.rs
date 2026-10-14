// SPDX-License-Identifier: MPL-2.0

//! **The Android app's notices**: its native library's crates, the faces it
//! bundles, and the Maven libraries Gradle packages beside them.
//!
//! The crates and the bundled components come from the same generator as the
//! desktop's, walked for the two ABIs the APK carries. The Maven libraries are
//! the part only Gradle can see: its `koruNotices` task resolves the variant's
//! runtime classpath and writes, for each library, the licenses its POM
//! declares and the license files the artifact itself carries
//! (`META-INF/LICENSE*`, and Google Play services' `third_party_licenses`).
//! This module turns that list into the file's third part.
//!
//! Gradle calls `koru-release notices android` while it builds the APK and
//! packages the result as `assets/licenses/THIRD-PARTY-NOTICES.txt`, which the
//! app's Settings → Open-source licenses screen shows.

use super::{Build, Bundled, CrateNotice, SPDX_DIR, Unrecorded, generate, parse_record, txt_files};
use serde::Deserialize;
use std::fs;
use std::path::{Path, PathBuf};

/// The workspace packages whose code is in the APK: the JNI library, and
/// through it everything it links.
pub(crate) const PACKAGES: &[&str] = &["koru-mobile-native"];

/// The ABIs `app/build.gradle.kts` builds the native library for.
pub(crate) const TRIPLES: &[&str] = &["aarch64-linux-android", "x86_64-linux-android"];

/// The license Google Play services' POMs declare. It is proprietary, and
/// accepted for Android (the open-source plan, "Licensing"): the app uses it
/// for QR scanning and Google sign-in. Its terms are not reproduced; the
/// notice says what it is and where they are.
pub(crate) const ANDROID_SDK: &str = "LicenseRef-Android-SDK-License";

/// The terms ML Kit's POMs declare, which the code scanner pulls in. The same
/// kind of thing as [`ANDROID_SDK`]: proprietary, Google's, accepted.
pub(crate) const ML_KIT: &str = "LicenseRef-ML-Kit-Terms";

/// What the notices say for a proprietary Google library, by its license.
fn proprietary_text(id: &str) -> Option<&'static str> {
    match id {
        ANDROID_SDK => Some(
            "Proprietary. Google Play services and the other Google libraries marked with \
             this license are not open source. Google distributes them under the Android \
             Software Development Kit License Agreement, https://developer.android.com/studio/terms, \
             and Koru carries them unmodified. The third-party notices they include are \
             reproduced in this part under their names.",
        ),
        ML_KIT => Some(
            "Proprietary. The ML Kit libraries the code scanner uses are not open source. \
             Google distributes them under the ML Kit Terms of Service, \
             https://developers.google.com/ml-kit/terms, and Koru carries them unmodified. \
             The third-party notices they include are reproduced in this part under their names.",
        ),
        _ => None,
    }
}

/// One Maven library, as Gradle's `koruNotices` task writes it.
#[derive(Debug, Clone, Deserialize)]
pub(crate) struct Library {
    /// `group:artifact:version`.
    pub coordinates: String,
    /// The POM's `<url>`, when it has one.
    #[serde(default)]
    pub url: Option<String>,
    /// The POM's `<licenses>`, or its nearest parent's.
    #[serde(default)]
    pub licenses: Vec<PomLicense>,
    /// License files inside the artifact, by their path in it.
    #[serde(default)]
    pub files: Vec<File>,
}

#[derive(Debug, Clone, Deserialize)]
pub(crate) struct PomLicense {
    #[serde(default)]
    pub name: Option<String>,
    #[serde(default)]
    pub url: Option<String>,
}

#[derive(Debug, Clone, Deserialize)]
pub(crate) struct File {
    pub name: String,
    pub text: String,
}

/// The SPDX identifier (or [`ANDROID_SDK`]) a POM license names, by its name
/// or its URL. POMs spell the same license a dozen ways; an unknown one
/// returns `None`, and the build refuses the library until it is added here.
pub(crate) fn license_id(license: &PomLicense) -> Option<&'static str> {
    let name = license.name.as_deref().unwrap_or("").to_ascii_lowercase();
    let url = license.url.as_deref().unwrap_or("").to_ascii_lowercase();
    if name.contains("android software development kit license")
        || url.contains("developer.android.com/studio/terms")
    {
        Some(ANDROID_SDK)
    } else if name == "ml kit terms of service"
        || url.contains("developers.google.com/ml-kit/terms")
    {
        Some(ML_KIT)
    } else if (name.contains("apache") && name.contains("2"))
        || name == "apache-2.0"
        || url.contains("apache.org/licenses/license-2.0")
    {
        Some("Apache-2.0")
    } else if name == "mit" || name == "the mit license" || name == "mit license" {
        Some("MIT")
    } else if name.contains("bsd-3-clause")
        || name == "bsd 3-clause"
        || name == "the 3-clause bsd license"
        || url.contains("opensource.org/licenses/bsd-3-clause")
    {
        Some("BSD-3-Clause")
    } else {
        None
    }
}

/// The notices for `libraries`, sorted by coordinates.
///
/// - **Files the artifact carries are copied verbatim**, as a crate's are.
///   Google Play services packs the notices of what it contains into one
///   `third_party_licenses.txt` indexed by `third_party_licenses.json`; those
///   are split back into one text per component.
/// - **A library carrying none uses its record in `licences/maven/`**, the
///   license file from its repository at the release (protobuf's jars carry
///   none), keyed by `group:artifact`.
/// - **Otherwise it gets the standard text of the license its POM
///   declares**, which for Apache-2.0 (no copyright line in the text) is the
///   whole of what that license asks. A library whose POM declares a license
///   that needs a copyright line, and that has no file and no record, is
///   refused: there is nobody to name.
/// - **Google's proprietary licenses** ([`ANDROID_SDK`], [`ML_KIT`]) get a
///   statement naming the terms, beside the third-party texts the library
///   carries.
/// - **A POM license this module does not know is refused**, files or not.
pub(crate) fn notices(root: &Path, libraries: &[Library]) -> Result<Vec<CrateNotice>, String> {
    let records = if libraries.is_empty() {
        Vec::new()
    } else {
        read_maven_records(&root.join(MAVEN_DIR))?
    };
    let mut out = Vec::with_capacity(libraries.len());
    let mut refused = Vec::new();
    for library in libraries {
        let (name, version) = library
            .coordinates
            .rsplit_once(':')
            .ok_or_else(|| format!("`{}` is not group:artifact:version", library.coordinates))?;
        let ids: Vec<Option<&'static str>> = library.licenses.iter().map(license_id).collect();
        // **Every license a POM names must be known**, even when the
        // artifact carries its own texts: a proprietary license we do not
        // recognize would otherwise ship unnamed.
        if ids.is_empty() || ids.contains(&None) {
            refused.push(format!("{} ({})", library.coordinates, declared(library)));
            continue;
        }
        let mut texts = embedded(&library.files)?;
        for id in ids.iter().flatten().rev() {
            if let Some(text) = proprietary_text(id) {
                texts.insert(0, ((*id).to_owned(), text.to_owned()));
            }
        }
        let record = records.iter().find(|record| {
            record
                .field("Libraries")
                .is_some_and(|names| names.split(", ").any(|known| known == name))
        });
        if let (true, Some(record)) = (texts.is_empty(), record) {
            let field = |key| record.field(key).unwrap_or_default();
            texts.push((
                format!(
                    "{} — from upstream at {} (checked at {})",
                    field("File"),
                    field("Source"),
                    field("Checked")
                ),
                record.text.clone(),
            ));
        }
        if texts.is_empty() {
            match ids.iter().flatten().find(|id| **id == "Apache-2.0") {
                Some(id) => {
                    let path = root.join(SPDX_DIR).join(format!("{id}.txt"));
                    let text = fs::read_to_string(&path)
                        .map_err(|error| format!("{}: {error}", path.display()))?;
                    texts.push((
                        format!("{id} — standard text of the license its POM declares"),
                        text,
                    ));
                }
                None => refused.push(format!("{} ({})", library.coordinates, declared(library))),
            }
        }
        out.push(CrateNotice {
            name: name.to_owned(),
            version: version.to_owned(),
            license: ids
                .iter()
                .flatten()
                .copied()
                .collect::<Vec<_>>()
                // A POM's list is not an SPDX expression: it does not say
                // whether its licenses are a choice or all apply.
                .join(", "),
            location: library
                .url
                .clone()
                .unwrap_or_else(|| format!("Maven: {}", library.coordinates)),
            texts,
            inferred: false,
        });
    }
    if !refused.is_empty() {
        return Err(format!(
            "these Android libraries declare a license koru-release does not know, or \
             carry no license file and have no record in {MAVEN_DIR} for a license whose \
             standard text needs a copyright line: {}",
            refused.join(", ")
        ));
    }
    out.sort_by(|a, b| (&a.name, &a.version).cmp(&(&b.name, &b.version)));
    Ok(out)
}

/// Where a Maven library's license is recorded when its artifact has none.
const MAVEN_DIR: &str = "crates/koru-release/licences/maven";

/// Every record in `licences/maven/`, each naming the `group:artifact`s it
/// covers, and checked like a crate's.
pub(crate) fn read_maven_records(dir: &Path) -> Result<Vec<Bundled>, String> {
    txt_files(dir)?
        .iter()
        .map(|path| {
            let text =
                fs::read_to_string(path).map_err(|error| format!("{}: {error}", path.display()))?;
            let record = parse_record(
                &text,
                &[
                    "Libraries",
                    "Checked",
                    "License",
                    "Disposition",
                    "File",
                    "Source",
                ],
            )
            .map_err(|error| format!("{}: {error}", path.display()))?;
            if record.field("Disposition") != Some("upstream") {
                return Err(format!(
                    "{}: a Maven record is upstream's own file",
                    path.display()
                ));
            }
            Ok(record)
        })
        .collect()
}

fn declared(library: &Library) -> String {
    let names: Vec<String> = library
        .licenses
        .iter()
        .map(|license| {
            license
                .name
                .clone()
                .or_else(|| license.url.clone())
                .unwrap_or_default()
        })
        .collect();
    if names.is_empty() {
        "no license in its POM".to_owned()
    } else {
        names.join(", ")
    }
}

/// The license texts an artifact carries, as `(label, text)`.
fn embedded(files: &[File]) -> Result<Vec<(String, String)>, String> {
    let index = files
        .iter()
        .find(|file| file.name == "third_party_licenses.json");
    let mut out = Vec::new();
    for file in files {
        match file.name.as_str() {
            "third_party_licenses.json" => {}
            "third_party_licenses.txt" => {
                let index = index.ok_or("third_party_licenses.txt without its .json index")?;
                out.extend(split_third_party(&index.text, &file.text)?);
            }
            _ => out.push((file.name.clone(), file.text.clone())),
        }
    }
    Ok(out)
}

#[derive(Deserialize)]
struct Span {
    start: usize,
    length: usize,
}

/// Splits Google Play services' `third_party_licenses.txt` by its index,
/// `{"<component>": {"start": n, "length": n}, …}`, byte offsets. Each text
/// follows a `<component>:` heading (then a blank line, or nothing, depending
/// on the library), which is how a span is checked: one that does not follow
/// its heading, or does not fit the text, is an error rather than a notice cut
/// in the wrong place. Components come out in name order.
pub(crate) fn split_third_party(index: &str, text: &str) -> Result<Vec<(String, String)>, String> {
    let spans: std::collections::BTreeMap<String, Span> = serde_json::from_str(index)
        .map_err(|error| format!("third_party_licenses.json: {error}"))?;
    let mut out = Vec::with_capacity(spans.len());
    for (component, span) in spans {
        let heading = format!("{component}:");
        let (before, slice) = text
            .get(..span.start)
            .zip(text.get(span.start..span.start + span.length))
            .ok_or_else(|| {
                format!("third_party_licenses: {component}'s span does not fit the text")
            })?;
        if !before.trim_end().ends_with(&heading) {
            return Err(format!(
                "third_party_licenses: {component}'s span does not follow its heading"
            ));
        }
        out.push((
            format!("third_party_licenses: {component}"),
            slice.to_owned(),
        ));
    }
    Ok(out)
}

const USAGE: &str = "usage: koru-release notices android --out <file> [--maven <libraries.json>] \
[--source <url>] [--debug]";

/// `koru-release notices android …`, run by Gradle.
///
/// `--debug` labels a crate that has no record in `licences/crates/` instead
/// of refusing it (see [`Unrecorded`]); a release build never passes it.
pub(crate) fn run(root: &Path, mut args: impl Iterator<Item = String>) -> Result<(), String> {
    if args.next().as_deref() != Some("android") {
        return Err(USAGE.into());
    }
    let mut out: Option<PathBuf> = None;
    let mut maven: Option<PathBuf> = None;
    let mut source: Option<String> = None;
    let mut unrecorded = Unrecorded::Refuse;
    while let Some(arg) = args.next() {
        match arg.as_str() {
            "--out" => out = Some(args.next().ok_or(USAGE)?.into()),
            "--maven" => maven = Some(args.next().ok_or(USAGE)?.into()),
            "--source" => source = Some(args.next().ok_or(USAGE)?),
            "--debug" => unrecorded = Unrecorded::Label,
            _ => return Err(format!("unexpected argument: {arg}\n{USAGE}")),
        }
    }
    let out = out.ok_or(USAGE)?;
    let libraries: Vec<Library> = match maven {
        Some(path) => serde_json::from_slice(
            &fs::read(&path).map_err(|error| format!("{}: {error}", path.display()))?,
        )
        .map_err(|error| format!("{}: {error}", path.display()))?,
        None => Vec::new(),
    };
    let build = Build {
        packages: PACKAGES,
        triples: TRIPLES.iter().map(|triple| (*triple).to_owned()).collect(),
        os: "android",
        unrecorded,
        source: source.as_deref(),
    };
    let text = generate(root, &build, &libraries)?;
    if let Some(parent) = out.parent() {
        fs::create_dir_all(parent).map_err(|error| error.to_string())?;
    }
    fs::write(&out, text).map_err(|error| format!("{}: {error}", out.display()))
}

#[cfg(test)]
mod tests;
