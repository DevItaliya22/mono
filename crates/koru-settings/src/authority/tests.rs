// SPDX-License-Identifier: MPL-2.0

//! The authority's behavior, and the one disk proof this crate is allowed.
//!
//! **Every path here comes from `SettingsPaths::under_root(temp)`.** Nothing
//! calls `discover`, so no test can reach a real preferences or local-app-data
//! path even by accident — which is the whole reason the rooted injection
//! exists (survey note 3). The staged-write-and-rename proof is the case
//! `TESTING.md`'s third exception names: what happens on disk *is* the thing
//! under test there, and it stays minimal.

use std::sync::atomic::{AtomicUsize, Ordering};

use super::*;
use crate::values::{CaretColorChoice, DensityChoice, FontFamily, FontSetting, ThemeChoice};

fn authority() -> (tempfile::TempDir, SettingsAuthority) {
    let root = tempfile::tempdir().unwrap();
    let authority = SettingsAuthority::new(SettingsPaths::under_root(root.path()));
    (root, authority)
}

#[derive(Default)]
struct RecordingSink {
    published: Mutex<Vec<Section>>,
}

impl SettingsEventSink for RecordingSink {
    fn publish(&self, section: Section) {
        self.published.lock().unwrap().push(section);
    }
}

struct RefusingAutostart;

impl StartOnLoginPort for RefusingAutostart {
    fn set(&self, _enabled: bool) -> Result<(), String> {
        Err("the startup entry could not be written".into())
    }
}

#[derive(Default)]
struct CountingAutostart {
    calls: AtomicUsize,
}

impl StartOnLoginPort for CountingAutostart {
    fn set(&self, _enabled: bool) -> Result<(), String> {
        self.calls.fetch_add(1, Ordering::Relaxed);
        Ok(())
    }
}

#[derive(Default)]
struct RecordingAutostart {
    calls: Mutex<Vec<bool>>,
    fail_rollback: bool,
}

impl StartOnLoginPort for RecordingAutostart {
    fn set(&self, enabled: bool) -> Result<(), String> {
        let mut calls = self.calls.lock().unwrap();
        calls.push(enabled);
        if self.fail_rollback && calls.len() == 2 {
            return Err("rollback refused".into());
        }
        Ok(())
    }
}

struct OrderedAutostart(Arc<Mutex<Vec<&'static str>>>);

impl StartOnLoginPort for OrderedAutostart {
    fn set(&self, _enabled: bool) -> Result<(), String> {
        self.0.lock().unwrap().push("startup");
        Ok(())
    }
}

#[test]
fn a_first_run_reads_the_registry_defaults() {
    let (_root, authority) = authority();
    assert_eq!(authority.global(), GlobalSettings::default());
    assert_eq!(authority.terminal(), TerminalSettings::default());
}

#[test]
fn a_patch_lands_and_reads_back() {
    let (_root, authority) = authority();
    let after = authority
        .patch_global(GlobalPatch {
            theme: Some(ThemeChoice::Dark),
            ..Default::default()
        })
        .unwrap();
    assert_eq!(after.theme, ThemeChoice::Dark);
    assert_eq!(authority.global().theme, ThemeChoice::Dark);
}

#[test]
fn two_patches_of_different_fields_both_land() {
    // The fix for survey note 4. Today's writers serialize the whole body, so
    // the second save would carry the first window's stale value for the field
    // it never touched.
    let (_root, authority) = authority();
    authority
        .patch_global(GlobalPatch {
            theme: Some(ThemeChoice::Dark),
            ..Default::default()
        })
        .unwrap();
    authority
        .patch_global(GlobalPatch {
            density: Some(DensityChoice::Compact),
            ..Default::default()
        })
        .unwrap();

    let settings = authority.global();
    assert_eq!(settings.theme, ThemeChoice::Dark);
    assert_eq!(settings.density, DensityChoice::Compact);
}

#[test]
fn concurrent_patches_of_different_fields_both_land() {
    let (_root, authority) = authority();
    let fields: Vec<GlobalPatch> = vec![
        GlobalPatch {
            theme: Some(ThemeChoice::Dark),
            ..Default::default()
        },
        GlobalPatch {
            density: Some(DensityChoice::Comfortable),
            ..Default::default()
        },
        GlobalPatch {
            caret_color: Some(CaretColorChoice::Amber),
            ..Default::default()
        },
        GlobalPatch {
            body_font: Some(FontSetting::Chosen(FontFamily::Lora)),
            ..Default::default()
        },
    ];
    std::thread::scope(|scope| {
        for patch in fields {
            let authority = authority.clone();
            scope.spawn(move || authority.patch_global(patch).unwrap());
        }
    });

    let settings = authority.global();
    assert_eq!(settings.theme, ThemeChoice::Dark);
    assert_eq!(settings.density, DensityChoice::Comfortable);
    assert_eq!(settings.caret_color, CaretColorChoice::Amber);
    assert_eq!(settings.body_font, FontSetting::Chosen(FontFamily::Lora));
}

#[test]
fn the_same_field_is_last_committed_wins() {
    let (_root, authority) = authority();
    authority
        .patch_global(GlobalPatch {
            theme: Some(ThemeChoice::Dark),
            ..Default::default()
        })
        .unwrap();
    let after = authority
        .patch_global(GlobalPatch {
            theme: Some(ThemeChoice::Light),
            ..Default::default()
        })
        .unwrap();
    assert_eq!(after.theme, ThemeChoice::Light);
    assert_eq!(authority.global().theme, ThemeChoice::Light);
}

#[test]
fn a_patch_that_spans_two_files_writes_both() {
    let (_root, authority) = authority();
    authority
        .patch_global(GlobalPatch {
            theme: Some(ThemeChoice::Dark),
            close_behavior: Some(crate::values::CloseBehaviorChoice::Quit),
            ..Default::default()
        })
        .unwrap();
    assert!(authority.paths().preferences_file().is_file());
    assert!(authority.paths().desktop_file().is_file());
    let settings = authority.global();
    assert_eq!(settings.theme, ThemeChoice::Dark);
    assert_eq!(
        settings.close_behavior,
        crate::values::CloseBehaviorChoice::Quit
    );
}

#[test]
fn a_patch_does_not_rewrite_a_file_it_does_not_touch() {
    // A theme change must not be able to fail on `desktop-v1.json`.
    let (_root, authority) = authority();
    authority
        .patch_global(GlobalPatch {
            theme: Some(ThemeChoice::Dark),
            ..Default::default()
        })
        .unwrap();
    assert!(authority.paths().preferences_file().is_file());
    assert!(
        !authority.paths().desktop_file().exists(),
        "the device-local file is untouched by a roaming-only patch"
    );
}

#[test]
fn an_empty_patch_changes_nothing_and_writes_nothing() {
    let (_root, authority) = authority();
    authority.patch_global(GlobalPatch::default()).unwrap();
    assert!(!authority.paths().preferences_file().exists());
    authority.patch_terminal(TerminalPatch::default()).unwrap();
    assert!(!authority.paths().terminal_file().exists());
}

#[test]
fn an_invalid_patch_is_refused_before_anything_is_staged() {
    let (_root, authority) = authority();
    let error = authority
        .patch_global(GlobalPatch {
            theme: Some(ThemeChoice::Dark),
            zoom: Some(42.0),
            ..Default::default()
        })
        .unwrap_err();
    assert!(matches!(error, PatchError::Invalid(_)));
    assert!(
        !authority.paths().preferences_file().exists(),
        "a refused patch leaves the disk exactly as it was — including the \
         valid fields that travelled with the invalid one"
    );
    assert_eq!(authority.global().theme, ThemeChoice::System);
}

#[test]
fn a_proportional_terminal_font_is_refused_rather_than_replaced() {
    let (_root, authority) = authority();
    let error = authority
        .patch_terminal(TerminalPatch {
            font: Some(FontSetting::Chosen(FontFamily::Lora)),
            ..Default::default()
        })
        .unwrap_err();
    match error {
        PatchError::Invalid(invalid) => assert_eq!(invalid.field, "font"),
        other => panic!("expected a validation refusal, got {other:?}"),
    }
}

#[test]
fn a_failed_write_reports_what_is_actually_true() {
    // The reconciliation case: a control must never pretend a value persisted.
    // A directory where the file should be is a write that cannot succeed, and
    // it is the failure that does not need a fault injector.
    let root = tempfile::tempdir().unwrap();
    let paths = SettingsPaths::under_root(root.path());
    std::fs::create_dir_all(paths.preferences_file()).unwrap();
    let authority = SettingsAuthority::new(paths);

    let error = authority
        .patch_global(GlobalPatch {
            theme: Some(ThemeChoice::Dark),
            ..Default::default()
        })
        .unwrap_err();
    match error {
        PatchError::Write { authoritative, .. } => assert_eq!(
            authoritative.theme,
            ThemeChoice::System,
            "the authoritative section is re-read, not assumed"
        ),
        other => panic!("expected a write failure, got {other:?}"),
    }
}

#[test]
fn a_failed_write_publishes_nothing() {
    let root = tempfile::tempdir().unwrap();
    let paths = SettingsPaths::under_root(root.path());
    std::fs::create_dir_all(paths.terminal_file()).unwrap();
    let authority = SettingsAuthority::new(paths);
    let sink = Arc::new(RecordingSink::default());
    authority.set_event_sink(sink.clone());

    authority
        .patch_terminal(TerminalPatch {
            theme: Some(ThemeChoice::Dark),
            ..Default::default()
        })
        .unwrap_err();
    assert!(
        sink.published.lock().unwrap().is_empty(),
        "an event is a fact; nothing happened, so nothing is said"
    );
}

#[test]
fn a_committed_change_publishes_its_section_once() {
    let (_root, authority) = authority();
    let sink = Arc::new(RecordingSink::default());
    authority.set_event_sink(sink.clone());

    authority
        .patch_global(GlobalPatch {
            theme: Some(ThemeChoice::Dark),
            ..Default::default()
        })
        .unwrap();
    authority
        .patch_terminal(TerminalPatch {
            zoom: Some(1.5),
            ..Default::default()
        })
        .unwrap();

    assert_eq!(
        *sink.published.lock().unwrap(),
        vec![Section::Global, Section::Terminal]
    );
}

#[test]
fn the_fact_is_published_only_after_the_bytes_are_readable() {
    // The ordering the whole design rests on: a window that reacts to a fact
    // and refetches never races the write it reports.
    struct RefetchingSink {
        authority: Mutex<Option<SettingsAuthority>>,
        seen: Mutex<Option<ThemeChoice>>,
    }
    impl SettingsEventSink for RefetchingSink {
        fn publish(&self, _section: Section) {
            let authority = self.authority.lock().unwrap().clone().unwrap();
            *self.seen.lock().unwrap() = Some(authority.global().theme);
        }
    }

    let (_root, authority) = authority();
    let sink = Arc::new(RefetchingSink {
        authority: Mutex::new(Some(authority.clone())),
        seen: Mutex::new(None),
    });
    authority.set_event_sink(sink.clone());
    authority
        .patch_global(GlobalPatch {
            theme: Some(ThemeChoice::Dark),
            ..Default::default()
        })
        .unwrap();

    assert_eq!(*sink.seen.lock().unwrap(), Some(ThemeChoice::Dark));
}

#[test]
fn start_on_login_does_not_commit_when_the_os_refuses() {
    let (_root, authority) = authority();
    authority.set_start_on_login_port(Arc::new(RefusingAutostart));
    let error = authority
        .patch_global(GlobalPatch {
            start_on_login: Some(true),
            ..Default::default()
        })
        .unwrap_err();
    match error {
        PatchError::SideEffect { field, .. } => assert_eq!(field, "start_on_login"),
        other => panic!("expected a side-effect refusal, got {other:?}"),
    }
    assert!(
        !authority.global().start_on_login,
        "a stored `true` the registry never got is a lie the person cannot see"
    );
    assert!(!authority.paths().desktop_file().exists());
}

#[test]
fn start_on_login_is_refused_when_no_host_can_perform_it() {
    let (_root, authority) = authority();
    let error = authority
        .patch_global(GlobalPatch {
            start_on_login: Some(true),
            ..Default::default()
        })
        .unwrap_err();
    assert!(matches!(error, PatchError::SideEffect { .. }));
}

#[test]
fn an_explicit_start_on_login_patch_always_reasserts_the_os() {
    let (_root, authority) = authority();
    let port = Arc::new(CountingAutostart::default());
    authority.set_start_on_login_port(port.clone());

    authority
        .patch_global(GlobalPatch {
            start_on_login: Some(false),
            ..Default::default()
        })
        .unwrap();
    assert_eq!(port.calls.load(Ordering::Relaxed), 1);

    authority
        .patch_global(GlobalPatch {
            start_on_login: Some(true),
            ..Default::default()
        })
        .unwrap();
    assert_eq!(port.calls.load(Ordering::Relaxed), 2);
    assert!(authority.global().start_on_login);
}

#[test]
fn failed_enable_restores_startup_and_keeps_prior_authority() {
    let port = Arc::new(RecordingAutostart::default());
    let before = GlobalSettings::default();
    let error = commit_with_startup(
        Some(StartupChange {
            port: Some(port.clone()),
            prior: false,
            wanted: true,
        }),
        before.clone(),
        || Err::<(), _>(std::io::Error::other("write refused")),
        || before.clone(),
    )
    .unwrap_err();
    assert!(matches!(error, PatchError::Write { .. }));
    assert_eq!(*port.calls.lock().unwrap(), [true, false]);
    assert!(!error.authoritative().unwrap().start_on_login);
}

#[test]
fn failed_disable_restores_startup_and_keeps_prior_authority() {
    let port = Arc::new(RecordingAutostart::default());
    let before = GlobalSettings {
        start_on_login: true,
        ..GlobalSettings::default()
    };
    let error = commit_with_startup(
        Some(StartupChange {
            port: Some(port.clone()),
            prior: true,
            wanted: false,
        }),
        before.clone(),
        || Err::<(), _>(std::io::Error::other("write refused")),
        || before.clone(),
    )
    .unwrap_err();
    assert!(matches!(error, PatchError::Write { .. }));
    assert_eq!(*port.calls.lock().unwrap(), [false, true]);
    assert!(error.authoritative().unwrap().start_on_login);
}

#[test]
fn failed_idempotent_reassert_needs_no_compensation() {
    let port = Arc::new(RecordingAutostart {
        fail_rollback: true,
        ..Default::default()
    });
    let before = GlobalSettings::default();
    let error = commit_with_startup(
        Some(StartupChange {
            port: Some(port.clone()),
            prior: false,
            wanted: false,
        }),
        before.clone(),
        || Err::<(), _>(std::io::Error::other("write refused")),
        || before.clone(),
    )
    .unwrap_err();
    assert!(matches!(error, PatchError::Write { .. }));
    assert_eq!(*port.calls.lock().unwrap(), [false]);
}

#[test]
fn rollback_failure_reports_the_disk_os_inconsistency() {
    let port = Arc::new(RecordingAutostart {
        fail_rollback: true,
        ..Default::default()
    });
    let before = GlobalSettings::default();
    let error = commit_with_startup(
        Some(StartupChange {
            port: Some(port.clone()),
            prior: false,
            wanted: true,
        }),
        before.clone(),
        || Err::<(), _>(std::io::Error::other("write refused")),
        || before.clone(),
    )
    .unwrap_err();
    assert_eq!(*port.calls.lock().unwrap(), [true, false]);
    assert!(matches!(error, PatchError::Inconsistent { .. }));
    assert!(error.message().contains("write refused"));
    assert!(error.message().contains("rollback refused"));

    let repaired = commit_with_startup(
        Some(StartupChange {
            port: Some(port.clone()),
            prior: false,
            wanted: false,
        }),
        before.clone(),
        || Ok(before.clone()),
        || before.clone(),
    )
    .unwrap();
    assert!(!repaired.start_on_login);
    assert_eq!(
        *port.calls.lock().unwrap(),
        [true, false, false],
        "retrying the disk-visible prior value must reassert and repair the OS"
    );
}

#[test]
fn startup_refusal_never_calls_persistence() {
    let persisted = AtomicUsize::new(0);
    let before = GlobalSettings::default();
    let error = commit_with_startup(
        Some(StartupChange {
            port: None,
            prior: false,
            wanted: true,
        }),
        before.clone(),
        || {
            persisted.fetch_add(1, Ordering::Relaxed);
            Ok(())
        },
        || before.clone(),
    )
    .unwrap_err();
    assert!(matches!(error, PatchError::SideEffect { .. }));
    assert_eq!(persisted.load(Ordering::Relaxed), 0);
}

#[test]
fn compensation_precedes_the_reconciliation_read() {
    let order = Arc::new(Mutex::new(Vec::new()));
    let port = Arc::new(OrderedAutostart(Arc::clone(&order)));
    let writes = Arc::clone(&order);
    let reads = Arc::clone(&order);
    let before = GlobalSettings::default();
    commit_with_startup(
        Some(StartupChange {
            port: Some(port),
            prior: false,
            wanted: true,
        }),
        before.clone(),
        move || {
            writes.lock().unwrap().push("persist");
            Err::<(), _>(std::io::Error::other("failed"))
        },
        move || {
            reads.lock().unwrap().push("reread");
            before
        },
    )
    .unwrap_err();
    assert_eq!(
        *order.lock().unwrap(),
        ["startup", "persist", "startup", "reread"]
    );
}

#[test]
fn reminder_startup_consent_commits_both_device_local_facts_together() {
    let (_root, authority) = authority();
    let port = Arc::new(CountingAutostart::default());
    authority.set_start_on_login_port(port.clone());

    let after = authority
        .patch_global(GlobalPatch {
            start_on_login: Some(true),
            reminder_start_on_login_explained: Some(true),
            ..Default::default()
        })
        .unwrap();

    assert_eq!(port.calls.load(Ordering::Relaxed), 1);
    assert!(after.start_on_login);
    assert!(after.reminder_start_on_login_explained);
    assert_eq!(authority.global(), after);
}

#[test]
fn refusing_reminder_startup_does_not_record_an_explanation() {
    let (_root, authority) = authority();
    authority.set_start_on_login_port(Arc::new(RefusingAutostart));

    authority
        .patch_global(GlobalPatch {
            start_on_login: Some(true),
            reminder_start_on_login_explained: Some(true),
            ..Default::default()
        })
        .unwrap_err();

    let after = authority.global();
    assert!(!after.start_on_login);
    assert!(!after.reminder_start_on_login_explained);
}

#[test]
fn automatic_update_checks_migrate_once_from_the_update_state_file() {
    let (root, authority) = authority();
    let state = root.path().join("updates").join("state.json");
    std::fs::create_dir_all(state.parent().unwrap()).unwrap();
    std::fs::write(
        &state,
        br#"{"automatic_checks":false,"last_seen_build":44,"etags":[]}"#,
    )
    .unwrap();

    assert!(
        !authority.global().automatic_update_checks,
        "the value is seeded from where it used to live"
    );

    // Once the authority has written its own file, the old one never votes
    // again — even if it changes.
    authority
        .patch_global(GlobalPatch {
            automatic_update_checks: Some(true),
            ..Default::default()
        })
        .unwrap();
    std::fs::write(&state, br#"{"automatic_checks":false}"#).unwrap();
    assert!(authority.global().automatic_update_checks);
}

#[test]
fn the_authority_never_writes_the_update_state_file() {
    // The split: the *preference* moves out, the *state* stays. Two writers
    // each writing a whole JSON body is exactly the clobber this packet
    // removes, so the authority reads that file and never touches it.
    let (root, authority) = authority();
    let state = root.path().join("updates").join("state.json");
    std::fs::create_dir_all(state.parent().unwrap()).unwrap();
    let original = br#"{"automatic_checks":true,"last_seen_build":44}"#;
    std::fs::write(&state, original).unwrap();

    authority
        .patch_global(GlobalPatch {
            automatic_update_checks: Some(false),
            ..Default::default()
        })
        .unwrap();

    assert_eq!(std::fs::read(&state).unwrap(), original);
    let desktop: serde_json::Value =
        serde_json::from_slice(&std::fs::read(authority.paths().desktop_file()).unwrap()).unwrap();
    assert_eq!(desktop["automatic_checks"], false);
}

#[test]
fn the_terminal_section_seeds_from_the_window_row_only_until_it_has_a_file() {
    let (root, authority) = authority();
    std::fs::write(
        root.path().join("windows.conf"),
        "terminal = theme=dark;zoom=1.4;cursor_blink=on\n",
    )
    .unwrap();
    assert_eq!(authority.terminal().theme, ThemeChoice::Dark);

    authority
        .patch_terminal(TerminalPatch {
            theme: Some(ThemeChoice::Light),
            ..Default::default()
        })
        .unwrap();
    // The row changes; the roaming file is now the authority and ignores it.
    std::fs::write(
        root.path().join("windows.conf"),
        "terminal = theme=dark;zoom=2.0\n",
    )
    .unwrap();
    let settings = authority.terminal();
    assert_eq!(settings.theme, ThemeChoice::Light);
    assert_eq!(
        settings.zoom.get(),
        1.4,
        "the seeded values persisted into the roaming file"
    );
}

/// **The disk proof.** `TESTING.md`'s third exception, and deliberately the
/// only test in this crate that inspects what is on the filesystem rather than
/// what a codec produced.
#[test]
fn a_write_publishes_by_rename_and_leaves_no_staging_file() {
    let (root, authority) = authority();
    authority
        .patch_global(GlobalPatch {
            theme: Some(ThemeChoice::Dark),
            ..Default::default()
        })
        .unwrap();

    let destination = authority.paths().preferences_file();
    assert!(destination.is_file());
    let staging = crate::paths::staging_path(destination);
    assert!(
        !staging.exists(),
        "the staging file is renamed over the destination, never left behind"
    );
    let leftovers: Vec<_> = std::fs::read_dir(root.path())
        .unwrap()
        .filter_map(Result::ok)
        .map(|entry| entry.file_name().to_string_lossy().into_owned())
        .filter(|name| name.contains(".part"))
        .collect();
    assert!(leftovers.is_empty(), "left {leftovers:?} behind");

    // And the destination holds a complete file, not a prefix of one.
    let body = std::fs::read_to_string(destination).unwrap();
    assert!(body.starts_with("# Koru preferences."));
    assert!(body.ends_with('\n'));
    assert!(body.contains("theme=dark"));
}

#[test]
fn a_staging_name_is_unique_to_this_process() {
    // Two hosts staging at once must not write the same temporary file.
    let staging = crate::paths::staging_path(std::path::Path::new("/tmp/preferences.conf"));
    let name = staging.file_name().unwrap().to_string_lossy().into_owned();
    assert!(name.ends_with(".part"));
    assert!(name.contains(&std::process::id().to_string()));
}

/// **The folder a person chose for a run survives the process that chose it**
/// (ADR-365). The window sends one field; a second authority over the same
/// files — which is what the next launch is — reads it back.
///
/// `null` puts it back to the platform's temporary directory, which is the
/// third state a `Field` exists for: absent, cleared and set are three
/// instructions, and only the middle one has no `Option` to live in.
#[test]
fn a_chosen_delegate_folder_is_stored_and_read_back_by_the_next_launch() {
    let root = tempfile::tempdir().unwrap();
    let paths = SettingsPaths::under_root(root.path());
    let authority = SettingsAuthority::new(paths.clone());

    assert_eq!(authority.global().delegate_temporary_root, None);

    let chosen = authority
        .patch_global(GlobalPatch {
            delegate_temporary_root: crate::Field::Set("D:/work/koru-runs".into()),
            ..Default::default()
        })
        .unwrap();
    assert_eq!(
        chosen.delegate_temporary_root.as_deref(),
        Some("D:/work/koru-runs")
    );

    let next_launch = SettingsAuthority::new(paths);
    assert_eq!(
        next_launch.global().delegate_temporary_root.as_deref(),
        Some("D:/work/koru-runs"),
        "the choice is what the next launch, and the run worker, start from"
    );

    next_launch
        .patch_global(GlobalPatch {
            delegate_temporary_root: crate::Field::Clear,
            ..Default::default()
        })
        .unwrap();
    assert_eq!(
        SettingsAuthority::new(SettingsPaths::under_root(root.path()))
            .global()
            .delegate_temporary_root,
        None,
        "cleared is back to following this machine, not a folder called nothing"
    );
}

/// A patch that names only the folder leaves the roaming file alone: where a
/// run works is about this machine, and it must not be able to fail on — or
/// rewrite — the preferences that follow the person.
#[test]
fn choosing_a_delegate_folder_writes_only_the_device_file() {
    let (_root, authority) = authority();
    authority
        .patch_global(GlobalPatch {
            delegate_temporary_root: crate::Field::Set("D:/work/koru-runs".into()),
            ..Default::default()
        })
        .unwrap();
    assert!(authority.paths().desktop_file().is_file());
    assert!(!authority.paths().preferences_file().exists());
}
