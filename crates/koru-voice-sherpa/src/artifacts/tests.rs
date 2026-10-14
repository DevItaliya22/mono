// SPDX-License-Identifier: MPL-2.0

use super::*;

/// `sherpa-onnx-sys`' own `archive_name`, transcribed from the build script of
/// the pinned version, as patched by `third-party/patches/`. That patch names
/// upstream's TTS-free `…-no-tts-lib` archive for every static target, and
/// refuses Linux aarch64, for which upstream publishes none.
///
/// **The point of a second copy is that it is a second copy.** The names are
/// compiled into a dependency's build script where nothing of ours can read
/// them, and the failure they produce — "SHERPA_ONNX_ARCHIVE_DIR does not
/// contain expected archive" — arrives on somebody's build machine rather than
/// here. This function is the shape of the rule; the test below checks our
/// table against it.
fn upstream_archive_name(target_os: &str, target_arch: &str) -> Option<String> {
    let stem = match (target_os, target_arch) {
        ("windows", "x86_64") => "win-x64-static-MT-Release-no-tts-lib",
        ("windows", "aarch64") => "win-arm64-static-MT-Release-no-tts-lib",
        ("macos", "aarch64") => "osx-arm64-static-no-tts-lib",
        ("macos", "x86_64") => "osx-x64-static-no-tts-lib",
        ("linux", "x86_64") => "linux-x64-static-no-tts-lib",
        _ => return None,
    };
    Some(format!("sherpa-onnx-v{SHERPA_ONNX_VERSION}-{stem}.tar.bz2"))
}

#[test]
fn every_name_matches_the_rule_the_build_script_uses() {
    for artifact in &NAMES {
        let upstream = upstream_archive_name(artifact.target_os, artifact.target_arch)
            .expect("every target in NAMES is one sherpa-onnx-sys publishes for");
        assert_eq!(
            artifact.file_name(),
            upstream,
            "{} would make sherpa-onnx-sys look for a file the provisioning \
             script never placed",
            artifact.target
        );
    }
}

#[test]
fn the_table_is_the_three_targets_koru_release_ships() {
    // Windows on ARM is out of the gate by construction rather than by choice:
    // `koru-release`'s index knows exactly these three, and an artifact for a
    // target we do not ship has no business being provisioned.
    let targets: Vec<&str> = NAMES.iter().map(|a| a.target).collect();
    assert_eq!(
        targets,
        [
            "x86_64-pc-windows-msvc",
            "aarch64-apple-darwin",
            "x86_64-apple-darwin"
        ]
    );
}

#[test]
fn every_artifact_is_pinned_to_a_hash() {
    // An unpinned artifact is the hole this table exists to close: the
    // provisioning script would place bytes nothing checked, which is the same
    // failure as trusting a catalog's digest.
    for artifact in &NAMES {
        assert!(
            artifact.is_pinned(),
            "{} has no sha256, so nothing verifies what is downloaded",
            artifact.target
        );
        assert!(
            artifact
                .sha256
                .chars()
                .all(|c| c.is_ascii_hexdigit() && !c.is_ascii_uppercase()),
            "{} carries something that is not a lowercase hex digest",
            artifact.target
        );
        assert!(artifact.size > 0, "{} declares no size", artifact.target);
    }
}

#[test]
fn a_url_names_the_pinned_version() {
    let mac = artifact_for("macos", "aarch64").expect("macOS arm64 is a shipped target");
    assert_eq!(
        mac.url(),
        format!(
            "https://github.com/k2-fsa/sherpa-onnx/releases/download/v{SHERPA_ONNX_VERSION}/\
             sherpa-onnx-v{SHERPA_ONNX_VERSION}-osx-arm64-static-no-tts-lib.tar.bz2"
        )
    );
}

/// **Every shipped target links upstream's TTS-free archive.** The ordinary
/// static archive carries eSpeak NG (GPL-3.0). If any row ever pointed back at
/// it, the binaries that link the voice runtime would be GPL works again.
#[test]
fn every_target_links_upstreams_tts_free_archive() {
    for artifact in &NAMES {
        assert!(
            artifact.stem.ends_with("-no-tts-lib") && artifact.stem.contains("-static-"),
            "{} links {}, which is not a TTS-free static archive",
            artifact.target,
            artifact.stem
        );
    }
}

#[test]
fn a_target_we_do_not_ship_has_no_artifact() {
    // Not an error and not a panic: Linux is a target somebody can build for,
    // and the honest answer is that this product provisions nothing for it.
    assert!(artifact_for("linux", "x86_64").is_none());
    assert!(artifact_for("windows", "aarch64").is_none());
}

#[test]
fn the_environment_variable_names_are_sherpas_own() {
    // Two spellings that have to match a dependency's build script exactly. A
    // typo in either is a silent download rather than an error, which is the
    // whole failure this plan exists to prevent.
    assert_eq!(env::LIB_DIR, "SHERPA_ONNX_LIB_DIR");
    assert_eq!(env::ARCHIVE_DIR, "SHERPA_ONNX_ARCHIVE_DIR");
}
