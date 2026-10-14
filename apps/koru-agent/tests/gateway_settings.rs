// SPDX-License-Identifier: MPL-2.0

//! **Two windows, one settings authority, a real gateway process.**
//!
//! The unit tests prove each half: `koru-settings` proves the files round-trip
//! and a patch never clobbers a field it did not name, the gateway's own tests
//! prove the scope matrix, and the client's prove the classifier and the
//! bounded wait. None of them proves what SP-6 actually asks for — that a
//! settings change made by *one* window reaches *another* without either of
//! them polling, and that a token which may read cannot write.
//!
//! The gateway is `koru-agent.exe --worker gateway`, started by
//! [`gateway_process`] and spawned with `--root`, so every file it touches is
//! under a temp directory. **No test here resolves a real preferences path**;
//! that is the point of the rooted injection this packet added.
//!
//! Nothing waits on a timer for an answer to become true. Each wait is bounded
//! and fails loudly: a test that sleeps and then asserts is a test that passes
//! on a fast machine.

mod gateway_process;

use std::path::Path;
use std::time::{Duration, Instant};

use koru_api_client::Client;

use gateway_process::Gateway;
use koru_settings::{
    CaretColorChoice, DensityChoice, FontFamily, FontSetting, GlobalPatch, Section, TerminalPatch,
    ThemeChoice,
};

const DELIVERY: Duration = Duration::from_secs(20);

fn hello(product: &str, scopes: Vec<String>) -> koru_api_discovery::ClientHello {
    koru_api_discovery::ClientHello {
        product: product.into(),
        client_version: env!("CARGO_PKG_VERSION").into(),
        segment: koru_channel::segment().into(),
        api_version: koru_api_wire::API_VERSION,
        requested_scopes: scopes,
        grant: None,
    }
}

fn connect(root: &Path, product: &str, scopes: Vec<String>) -> Client {
    let found =
        koru_api_discovery::connect_at(koru_channel::segment(), root, &hello(product, scopes))
            .expect("connect to the running gateway");
    Client::from_gateway(found).expect("build gateway client")
}

/// A first-party window: it asks for the scopes it needs by name, because
/// asking filters rather than adds.
fn window(root: &Path, product: &str) -> Client {
    connect(
        root,
        product,
        vec![
            koru_settings::SETTINGS_READ_SCOPE.into(),
            koru_settings::SETTINGS_WRITE_SCOPE.into(),
            "events.read".into(),
        ],
    )
}

fn start(root: &Path) -> Gateway {
    let mut gateway = Gateway::start(root);
    gateway.published(&hello("gateway-settings-integration", Vec::new()));
    gateway
}

fn within(what: &str, mut ready: impl FnMut() -> bool) {
    let deadline = Instant::now() + DELIVERY;
    while Instant::now() < deadline {
        if ready() {
            return;
        }
        std::thread::sleep(Duration::from_millis(10));
    }
    panic!("{what} did not happen within {DELIVERY:?}");
}

/// The whole of the invalidation contract, end to end.
#[test]
fn a_change_by_one_window_invalidates_the_section_in_another() {
    let root = tempfile::tempdir().expect("temp root");
    let _gateway = start(root.path());
    let a = window(root.path(), "settings-window-a");
    let b = window(root.path(), "settings-window-b");

    let watch = b
        .settings()
        .watch(|_section| {})
        .expect("subscribe to settings facts");
    let flags = watch.flags();
    assert!(!flags.is_stale(Section::Global));

    a.settings()
        .patch_global(&GlobalPatch {
            theme: Some(ThemeChoice::Dark),
            ..Default::default()
        })
        .expect("the first window writes");

    within("the change reached the second window", || {
        flags.is_stale(Section::Global)
    });
    assert!(
        !flags.is_stale(Section::Terminal),
        "a Global change does not make a window refetch its terminal settings"
    );

    // And the fact is *committed* before it is announced: by the time B is
    // told, B's own read already sees the change.
    assert_eq!(
        b.settings()
            .global()
            .expect("the second window reads")
            .theme,
        ThemeChoice::Dark
    );
    watch.close();
}

/// A read token reads and cannot write; a first-party token writes.
#[test]
fn a_default_credential_reads_the_settings_and_changes_none_of_them() {
    let root = tempfile::tempdir().expect("temp root");
    let _gateway = start(root.path());
    // No requested scopes at all: the ordinary default set, which is what MCP,
    // plugins and scripts hold.
    let reader = connect(root.path(), "settings-reader", Vec::new());

    let before = reader.settings().global().expect("a default token reads");
    assert_eq!(before.theme, ThemeChoice::System);
    assert_eq!(
        reader
            .settings()
            .terminal()
            .expect("and reads every section")
            .zoom
            .get(),
        1.0
    );

    let refused = reader
        .settings()
        .patch_global(&GlobalPatch {
            theme: Some(ThemeChoice::Dark),
            ..Default::default()
        })
        .expect_err("a default token must not change what every window looks like");
    assert!(
        refused.is_permission_denied(),
        "expected a permission refusal, got {refused}"
    );
    assert_eq!(
        reader.settings().global().expect("read again").theme,
        ThemeChoice::System,
        "and nothing moved"
    );

    // The same gateway, a token that asked for the write scope by name.
    window(root.path(), "settings-writer")
        .settings()
        .patch_global(&GlobalPatch {
            theme: Some(ThemeChoice::Dark),
            ..Default::default()
        })
        .expect("a first-party window writes");
    assert_eq!(
        reader.settings().global().expect("read once more").theme,
        ThemeChoice::Dark
    );
}

/// Two windows changing two different fields at the same moment both land.
///
/// This is survey note 4, gone. Before this authority, each writer serialized
/// the whole file, so the second save carried the first window's stale value
/// for the field it never touched.
#[test]
fn concurrent_different_field_writes_from_two_windows_both_land() {
    let root = tempfile::tempdir().expect("temp root");
    let _gateway = start(root.path());
    let a = window(root.path(), "settings-window-a");
    let b = window(root.path(), "settings-window-b");

    std::thread::scope(|scope| {
        scope.spawn(|| {
            a.settings()
                .patch_global(&GlobalPatch {
                    density: Some(DensityChoice::Compact),
                    ..Default::default()
                })
                .expect("A writes density");
        });
        scope.spawn(|| {
            b.settings()
                .patch_global(&GlobalPatch {
                    caret_color: Some(CaretColorChoice::Amber),
                    ..Default::default()
                })
                .expect("B writes the caret color");
        });
    });

    let settled = a.settings().global().expect("read back");
    assert_eq!(settled.density, DensityChoice::Compact);
    assert_eq!(settled.caret_color, CaretColorChoice::Amber);
}

/// The terminal section round-trips, including the value that is refused.
#[test]
fn the_terminal_section_round_trips_and_refuses_a_proportional_font() {
    let root = tempfile::tempdir().expect("temp root");
    let _gateway = start(root.path());
    let client = window(root.path(), "settings-terminal");

    let after = client
        .settings()
        .patch_terminal(&TerminalPatch {
            font: Some(FontSetting::Chosen(FontFamily::SourceCodePro)),
            zoom: Some(1.5),
            ..Default::default()
        })
        .expect("a monospaced family is accepted");
    assert_eq!(after.font, FontSetting::Chosen(FontFamily::SourceCodePro));
    assert_eq!(after.zoom.get(), 1.5);

    let refused = client
        .settings()
        .patch_terminal(&TerminalPatch {
            font: Some(FontSetting::Chosen(FontFamily::Lora)),
            ..Default::default()
        })
        .expect_err("a proportional family is refused, not silently replaced");
    assert!(!refused.is_permission_denied());
    assert_eq!(
        client.settings().terminal().expect("read back").font,
        FontSetting::Chosen(FontFamily::SourceCodePro),
        "a refused patch changes nothing"
    );
}

/// A window renders before it has asked anything, and corrects itself when the
/// answer lands.
#[test]
fn the_bounded_wait_hands_back_defaults_and_then_the_truth() {
    let root = tempfile::tempdir().expect("temp root");
    let _gateway = start(root.path());
    let writer = window(root.path(), "settings-writer");
    writer
        .settings()
        .patch_global(&GlobalPatch {
            theme: Some(ThemeChoice::Dark),
            ..Default::default()
        })
        .expect("something to find");

    let client = window(root.path(), "settings-starting-window");
    let mut pending = client.settings().global_when_ready();
    // The window has something to draw with immediately, on its own thread.
    assert_eq!(pending.defaults().theme, ThemeChoice::System);

    let deadline = Instant::now() + DELIVERY;
    let authoritative = loop {
        if let Some(answer) = pending.take() {
            break answer.expect("the section landed inside the budget");
        }
        assert!(Instant::now() < deadline, "the wait never resolved");
        std::thread::sleep(Duration::from_millis(5));
    };
    assert_eq!(authoritative.theme, ThemeChoice::Dark);
    assert!(pending.is_resolved());
    assert!(pending.take().is_none(), "the answer arrives once");
}

/// A rooted gateway keeps its own settings, and the files land where the
/// authority says.
#[test]
fn a_rooted_gateway_writes_only_under_its_root() {
    let root = tempfile::tempdir().expect("temp root");
    let _gateway = start(root.path());
    window(root.path(), "settings-writer")
        .settings()
        .patch_global(&GlobalPatch {
            theme: Some(ThemeChoice::Dark),
            ..Default::default()
        })
        .expect("write");

    let settings_dir = root.path().join("settings");
    let preferences = settings_dir.join("preferences.conf");
    within("the preferences file appeared under the root", || {
        preferences.is_file()
    });
    let body = std::fs::read_to_string(&preferences).expect("read it back");
    assert!(body.starts_with("# Koru preferences."));
    assert!(body.contains("version=2"));
    assert!(body.contains("theme=dark"));
    assert!(
        !settings_dir.join("state.json").exists(),
        "the authority reads koru-update's state file and never writes one"
    );
}
