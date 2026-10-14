// SPDX-License-Identifier: MPL-2.0

use super::*;

#[test]
fn every_name_appears_exactly_once() {
    for var in VARS {
        assert_eq!(
            VARS.iter().filter(|v| v.name == var.name).count(),
            1,
            "{} is declared twice",
            var.name
        );
    }
}

#[test]
fn every_entry_says_what_it_is_for() {
    for var in VARS {
        assert!(
            !var.purpose.trim().is_empty(),
            "{} has no purpose",
            var.name
        );
        assert!(!var.values.trim().is_empty(), "{} has no values", var.name);
        assert!(
            !var.affects.trim().is_empty(),
            "{} affects nothing?",
            var.name
        );
    }
}

#[test]
fn the_one_truthiness_rule() {
    // A private test-only variable; the registry deliberately does not list
    // it, and the source sweep below skips this file.
    const NAME: &str = "KORU_ENV_RULE_TEST";
    let case = |value: Option<&str>, expect: bool| {
        match value {
            // set_var is safe enough single-threaded within one test; the
            // suite's other tests do not read this name.
            Some(value) => unsafe { std::env::set_var(NAME, value) },
            None => unsafe { std::env::remove_var(NAME) },
        }
        assert_eq!(switch(NAME), expect, "value {value:?}");
    };
    case(None, false);
    case(Some(""), false);
    case(Some("0"), false);
    case(Some("false"), false);
    case(Some("No"), false);
    case(Some(" OFF "), false);
    case(Some("1"), true);
    case(Some("true"), true);
    case(Some("yes"), true);
    case(Some("anything"), true);
    case(None, false);
}

/// The two rules that are the whole value of a `.env`: a process variable
/// wins, and a key we do not know stops the boot rather than being skipped.
#[test]
fn an_env_file_refuses_what_it_does_not_understand() {
    let dir = tempfile::tempdir().expect("temp dir");
    let write = |body: &str| {
        let path = dir.path().join(".env");
        std::fs::write(&path, body).expect("written");
        path
    };

    // Comments, blanks, quotes, `export`, and spaces around the `=`.
    let path = write(
        "# a comment

KORU_TELEMETRY_BASE_URL = \"http://127.0.0.1:8787\"
export KORU_TELEMETRY_BATCH_EVENTS=10
",
    );
    let loaded = load_file(&path).expect("loads");
    assert_eq!(
        loaded.names,
        [
            name::KORU_TELEMETRY_BASE_URL,
            name::KORU_TELEMETRY_BATCH_EVENTS
        ]
    );
    assert_eq!(
        std::env::var(name::KORU_TELEMETRY_BASE_URL).as_deref(),
        Ok("http://127.0.0.1:8787"),
        "the quotes are stripped, not stored"
    );

    // Already set wins, and says so by not reporting the name.
    let path = write(
        "KORU_TELEMETRY_BASE_URL=https://m.koru.do
",
    );
    let loaded = load_file(&path).expect("loads");
    assert!(loaded.names.is_empty());
    assert_eq!(
        std::env::var(name::KORU_TELEMETRY_BASE_URL).as_deref(),
        Ok("http://127.0.0.1:8787"),
        "the process beats the file"
    );

    // A typo is named and refused. Skipping it is how a sink stays silently
    // off while somebody stares at an empty dashboard.
    let path = write(
        "KORU_TELEMETRY_TOKEN=oops
",
    );
    match load_file(&path) {
        Err(EnvFileError::Unknown { line, key }) => {
            assert_eq!((line, key.as_str()), (1, "KORU_TELEMETRY_TOKEN"));
        }
        other => panic!(
            "an unknown key must be refused, got {other:?}",
            other = other.map(|l| l.names)
        ),
    }
    let path = write(
        "this is not a pair
",
    );
    assert!(matches!(
        load_file(&path),
        Err(EnvFileError::Malformed { line: 1 })
    ));

    unsafe { std::env::remove_var(name::KORU_TELEMETRY_BASE_URL) };
    unsafe { std::env::remove_var(name::KORU_TELEMETRY_BATCH_EVENTS) };
}

/// A credential's value must not reach the listing. `koru.bat envs` is what
/// someone runs while something is misbehaving, and its output is what they
/// paste into an issue — so a variable marked secret has to render as "set",
/// never as itself.
#[test]
fn a_secret_never_renders_its_value() {
    let secrets: Vec<&EnvVar> = VARS.iter().filter(|var| var.secret).collect();
    assert!(
        !secrets.is_empty(),
        "if nothing is marked secret any more, delete the marker rather than this test"
    );
    for var in secrets {
        assert_eq!(
            var.scope,
            Scope::Runtime,
            "{} is not read at run time",
            var.name
        );
        // A value nothing else in the suite would produce, so a hit is this
        // variable and not a coincidence.
        const VALUE: &str = "a_secret_that_must_not_be_printed";
        unsafe { std::env::set_var(var.name, VALUE) };
        let table = table();
        unsafe { std::env::remove_var(var.name) };
        assert!(
            !table.contains(VALUE),
            "{} printed its value in the listing",
            var.name
        );
        assert!(
            table.contains("[currently set, hidden]"),
            "{} did not say it was set",
            var.name
        );
        // The document describes variables and never reads one, so it is
        // unchanged — this is what keeps reference/ENV.md
        // free of a per-machine difference.
        assert!(markdown().contains(var.name));
        assert!(!markdown().contains(VALUE));
    }
}

/// The drift test: a `KORU_*` variable read anywhere in the open-source tree
/// must be declared here, and every declared runtime/compile variable must
/// actually be read there. The same contract as `koru-keys`' registry tests —
/// a table that can drift from the code is a fifth copy of the truth.
///
/// **The hosted services are not swept** (ADR-456): a package marked
/// `LicenseRef-Koru-Proprietary` (the hosted Koru Cloud services, which are
/// not in this repository) declares what it alone reads in its own table and
/// holds that table to its sources with the same sweep.
#[test]
fn every_variable_read_in_the_tree_is_declared_and_vice_versa() {
    let root = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../..");
    if !root.join("crates").is_dir() || !root.join("apps").is_dir() {
        return; // packaged build; sources absent
    }
    let read_names = drift::reads(&root, |package, proprietary| {
        !proprietary && !package.ends_with("koru-env")
    });

    let declared: std::collections::BTreeSet<&str> = VARS.iter().map(|var| var.name).collect();

    for name in &read_names {
        assert!(
            declared.contains(name.as_str()),
            "{name} is read somewhere in the tree but not declared in koru-env's VARS — add a row (with its purpose) or the flag is invisible to `koru.bat envs` and reference/ENV.md"
        );
    }
    for var in VARS.iter().filter(|var| var.scope != Scope::Script) {
        // The sweep only recognises `KORU_`-prefixed names, so the reverse
        // check is scoped to them: RUST_LOG is read through tracing's
        // EnvFilter and CLOUDFLARE_API_TOKEN through its own literal.
        // KORU_UPDATE_ENV is read by this crate itself, which the sweep
        // excludes.
        if !var.name.starts_with("KORU_") || var.name == name::KORU_UPDATE_ENV {
            continue;
        }
        assert!(
            read_names.contains(var.name),
            "{} is declared but nothing in crates/ or apps/ outside the hosted services reads it — delete the row (or, if only a hosted service reads it, move it to that table: ADR-456), or the docs advertise a dead flag",
            var.name
        );
    }
}

/// `reference/ENV.md` is generated, never hand-edited. The same mechanism as
/// reference/SHORTCUTS.md: set the switch to rewrite, otherwise assert current.
#[test]
fn the_generated_env_table_is_up_to_date() {
    let path = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../../reference/ENV.md");
    let Some(parent) = path.parent() else { return };
    if !parent.is_dir() {
        return; // packaged build
    }
    let generated = markdown();
    if switch(name::KORU_UPDATE_ENV) {
        std::fs::write(&path, &generated).expect("write reference/ENV.md");
        return;
    }
    let current = std::fs::read_to_string(&path).unwrap_or_default();
    assert_eq!(
        current, generated,
        "reference/ENV.md is stale; regenerate with `KORU_UPDATE_ENV=1 cargo test -p koru-env`"
    );
}

#[test]
fn the_baked_values_are_the_production_file_set_and_the_environment_still_wins() {
    // The seven a release carries, and no secret among them — the Google
    // Docs importer's installed-app registration included (F8: configuration,
    // not confidentiality).
    assert_eq!(
        super::BAKED,
        [
            super::name::KORU_PUBLIC_API_ORIGIN,
            super::name::KORU_PUBLIC_API_ISSUER,
            super::name::KORU_PUBLIC_API_CREDENTIAL_ROLLOUT,
            super::name::KORU_SUPPORT_URL,
            super::name::KORU_FEEDBACK_URL,
            super::name::KORU_GOOGLE_DOCS_CLIENT_ID,
            super::name::KORU_GOOGLE_DOCS_CLIENT_SECRET,
        ]
    );
    for name in super::BAKED {
        let var = VARS
            .iter()
            .find(|var| var.name == name)
            .unwrap_or_else(|| panic!("{name} is baked but not declared"));
        assert!(
            !var.secret,
            "{name} is baked into every binary, so it cannot be a secret"
        );
    }
    // SAFETY-adjacent: a process-wide variable, set and removed within one test.
    let name = "KORU_ENV_TEST_BAKED_PROBE";
    unsafe { std::env::set_var(name, " probed ") };
    assert_eq!(super::value(name).as_deref(), Some("probed"));
    unsafe { std::env::remove_var(name) };
    assert_eq!(
        super::value(name),
        None,
        "a name nothing baked reads as absent"
    );
}

#[test]
fn a_blank_environment_value_falls_back_to_the_baked_one() {
    assert_eq!(super::resolve(None, None), None);
    assert_eq!(super::resolve(Some("   "), None), None);
    assert_eq!(
        super::resolve(
            Some(" https://env.example.test "),
            Some("https://baked.example.test")
        )
        .as_deref(),
        Some("https://env.example.test"),
        "the environment wins"
    );
    assert_eq!(
        super::resolve(Some(""), Some("https://baked.example.test")).as_deref(),
        Some("https://baked.example.test"),
        "a blank environment value is unset, not a choice of no Cloud"
    );
}

#[test]
fn cloud_is_configured_exactly_when_there_is_an_origin() {
    // SAFETY-adjacent: a process-wide variable, set and removed within one
    // test; nothing else in this crate's suite reads it.
    let name = super::name::KORU_PUBLIC_API_ORIGIN;
    unsafe { std::env::set_var(name, " https://cloud.example.test ") };
    assert_eq!(
        super::cloud_origin().as_deref(),
        Some("https://cloud.example.test")
    );
    assert!(super::cloud_configured());
    unsafe { std::env::remove_var(name) };
    // Without the environment the answer is whatever this build baked: a
    // community build bakes nothing, and is not configured.
    assert_eq!(
        super::cloud_configured(),
        option_env!("KORU_PUBLIC_API_ORIGIN").is_some_and(|value| !value.trim().is_empty())
    );
    assert_eq!(super::cloud_configured(), super::cloud_origin().is_some());
}
