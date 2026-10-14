// SPDX-License-Identifier: MPL-2.0

use super::*;
use koru_update::{PackageFormat, ReleaseNotes, UpdatePackage};

/// The two timestamp formats that are actually in the bucket. Build 1 was
/// written before the seconds-precision rule landed.
const NANOS: &str = "2026-08-05T04:54:53.664004300+00:00";
const SECONDS: &str = "2026-08-21T13:48:54Z";

const WINDOWS: &str = "windows-x86_64";
const MACOS: &str = "macos-aarch64";

fn entry(channel: ReleaseChannel, version: &str, build: u64, published: &str) -> ReleaseIndexEntry {
    entry_for(WINDOWS, channel, version, build, published)
}

fn entry_for(
    target: &str,
    channel: ReleaseChannel,
    version: &str,
    build: u64,
    published: &str,
) -> ReleaseIndexEntry {
    ReleaseIndexEntry {
        channel,
        version: version.into(),
        build_number: build,
        published_at: published.into(),
        commit: Some("abc1234".into()),
        title: format!("Release {version}"),
        summary: vec!["Something changed.".into()],
        notes_url: Some(format!("https://{HOST}/releases/{version}/index.html")),
        package_url: format!("https://{HOST}/releases/{version}/koru-{target}.zip"),
        size: 45_088_768,
        target: target.to_owned(),
    }
}

fn index_of(entries: Vec<ReleaseIndexEntry>) -> ReleaseIndex {
    ReleaseIndex {
        schema_version: SCHEMA_VERSION,
        entries,
    }
}

/// **The correction that took a network probe to find.** `published_at` is one
/// `Utc::now()` read *before* the build starts, so a long build can stamp an
/// older release later than a newer one — and the two live timestamp formats
/// do not even sort against each other as strings.
#[test]
fn newest_first_is_by_build_number_not_by_the_clock() {
    let index = index_of(vec![
        entry(ReleaseChannel::Nightly, "0.0.3-nightly.a", 4, SECONDS),
        // Older build, later clock: the trap.
        entry(
            ReleaseChannel::Nightly,
            "0.0.3-nightly.b",
            2,
            "2027-01-01T00:00:00Z",
        ),
        entry(ReleaseChannel::Nightly, "0.0.3-nightly.c", 9, NANOS),
    ]);

    let order: Vec<u64> = index
        .by_channel(ReleaseChannel::Nightly)
        .iter()
        .map(|row| row.build_number)
        .collect();
    assert_eq!(
        order,
        vec![9, 4, 2],
        "the clock is not the publication order"
    );
}

/// `.664004300+00:00` sorts before `Z` as a string because `.` is below `Z`,
/// while 53.664 is *after* 53. Both have to render as one date either way.
#[test]
fn both_published_timestamp_formats_read_as_one_date() {
    assert_eq!(
        entry(ReleaseChannel::Stable, "0.0.1", 1, NANOS).date(),
        "2026-08-05"
    );
    assert_eq!(
        entry(ReleaseChannel::Nightly, "0.0.3-nightly.a", 2, SECONDS).date(),
        "2026-08-21"
    );
    // A stamp nobody can parse still yields a row rather than taking the page
    // down with it.
    assert_eq!(
        entry(ReleaseChannel::Stable, "0.0.1", 1, "not a date at all").date(),
        "not a date"
    );
}

/// **One version, two platforms, two rows that can be told apart.**
///
/// A publish allocates a fresh build number from the shared counter, so
/// shipping 0.0.3 for Windows and then for macOS is two builds under two
/// `history/<channel>/<target>/` prefixes. They keep their own rows — each is a
/// separate download — and each row names its platform, or the second reads as
/// a duplicate of the first.
#[test]
fn a_version_published_for_two_platforms_is_two_rows_that_say_which() {
    let index = index_of(vec![
        entry_for(
            WINDOWS,
            ReleaseChannel::Nightly,
            "0.0.3-nightly.0",
            40,
            SECONDS,
        ),
        entry_for(
            MACOS,
            ReleaseChannel::Nightly,
            "0.0.3-nightly.0",
            41,
            SECONDS,
        ),
    ]);

    assert_eq!(index.by_channel(ReleaseChannel::Nightly).len(), 2);
    let panel = index.panel(ReleaseChannel::Nightly);
    assert!(panel.contains("Windows"), "{panel}");
    assert!(panel.contains("macOS (Apple silicon)"), "{panel}");
}

/// A target nobody has taught this page about is still a release somebody can
/// download, so it renders as itself rather than disappearing.
#[test]
fn an_unknown_target_names_itself_rather_than_vanishing() {
    let index = index_of(vec![entry_for(
        "linux-x86_64",
        ReleaseChannel::Nightly,
        "0.0.3-nightly.0",
        42,
        SECONDS,
    )]);
    assert!(
        index
            .panel(ReleaseChannel::Nightly)
            .contains("linux-x86_64")
    );
}

/// Entries written before macOS was published carry no target at all, and
/// every one of them is Windows. Losing that on a reload would relabel
/// thirty-odd live rows.
#[test]
fn an_entry_written_before_targets_existed_reads_back_as_windows() {
    let json = serde_json::json!({
        "channel": "nightly",
        "version": "0.0.3-nightly.0",
        "build_number": 33,
        "published_at": SECONDS,
        "title": "Release",
        "summary": ["Something changed."],
        "package_url": "https://updates.koru.do/releases/0.0.3/koru.zip",
        "size": 1,
    });
    let entry: ReleaseIndexEntry = serde_json::from_value(json).expect("an entry with no target");
    assert_eq!(entry.target, WINDOWS);
}

/// Verified live: `history/stable/.../1.json` and `history/beta/.../1.json` are
/// the same build and the same version. Beta really did serve it, so the Beta
/// tab shows it — and says where it came from, rather than passing a stable
/// build off as a beta.
#[test]
fn a_stable_release_appears_on_beta_and_says_where_it_came_from() {
    let index = index_of(vec![
        entry(ReleaseChannel::Stable, "0.0.1", 1, NANOS),
        entry(ReleaseChannel::Beta, "0.0.1", 1, NANOS),
        entry(ReleaseChannel::Beta, "0.0.2-beta.0", 2, SECONDS),
    ]);

    assert_eq!(index.by_channel(ReleaseChannel::Stable).len(), 1);
    assert_eq!(index.by_channel(ReleaseChannel::Beta).len(), 2);

    assert!(
        index
            .panel(ReleaseChannel::Beta)
            .contains("also released on stable")
    );
    assert!(
        !index
            .panel(ReleaseChannel::Stable)
            .contains("also released on stable"),
        "a stable release is not promoted to where it already was"
    );
    assert!(
        !index
            .row(&entry(ReleaseChannel::Beta, "0.0.2-beta.0", 2, SECONDS))
            .contains("also released on stable"),
        "a native beta version carries no tag"
    );
}

/// The key is `(channel, build_number)` and not the version, because one build
/// legitimately appears on two channels.
#[test]
fn upsert_replaces_the_same_build_on_the_same_channel() {
    let mut index = ReleaseIndex::default();
    index.upsert(entry(ReleaseChannel::Stable, "0.0.1", 1, NANOS));
    index.upsert(entry(ReleaseChannel::Stable, "0.0.1", 1, NANOS));
    assert_eq!(index.entries.len(), 1, "a rebuild is idempotent");

    index.upsert(entry(ReleaseChannel::Beta, "0.0.1", 1, NANOS));
    assert_eq!(
        index.entries.len(),
        2,
        "the same build on another channel is a second row"
    );

    let mut corrected = entry(ReleaseChannel::Stable, "0.0.1", 1, NANOS);
    corrected.title = "Corrected".into();
    index.upsert(corrected);
    assert_eq!(index.entries.len(), 2);
    assert_eq!(index.entries[0].title, "Corrected", "the row was replaced");
}

/// `+` is legal in a path and only means "space" in a query string. The live
/// nightly URLs carry it verbatim and resolve, so nothing may encode it.
#[test]
fn build_metadata_survives_into_the_link() {
    let row = index_of(vec![entry(
        ReleaseChannel::Nightly,
        "0.0.3-nightly.20260821+9656909",
        33,
        SECONDS,
    )])
    .html();

    assert!(row.contains("href=\"/releases/0.0.3-nightly.20260821+9656909/index.html\""));
    assert!(!row.contains("%2B"), "the plus was percent-encoded");
}

/// Root-absolute, because this page is served at two paths. A sibling-relative
/// href would resolve differently at `/releases/` than at
/// `/releases/index.html`.
#[test]
fn notes_href_is_root_absolute_so_it_works_at_both_paths() {
    let mut own = entry(ReleaseChannel::Stable, "0.0.1", 1, NANOS);
    assert_eq!(
        own.notes_href().as_deref(),
        Some("/releases/0.0.1/index.html")
    );

    // Another origin is passed through rather than mangled into a local path.
    own.notes_url = Some("https://example.com/notes.html".into());
    assert_eq!(
        own.notes_href().as_deref(),
        Some("https://example.com/notes.html")
    );

    // The manifest field is optional; the title is then plain text.
    own.notes_url = None;
    assert_eq!(own.notes_href(), None);
    assert!(!index_of(vec![own]).html().contains("<h3><a"));
}

#[test]
fn a_title_and_a_summary_line_are_escaped() {
    let mut hostile = entry(ReleaseChannel::Stable, "1.0.0", 1, SECONDS);
    hostile.title = "Faster & <script>alert(1)</script>".into();
    hostile.summary = vec!["A <b>bold</b> claim".into()];

    let rendered = index_of(vec![hostile]).html();
    assert!(!rendered.contains("<script>alert(1)</script>"));
    assert!(rendered.contains("Faster &amp; &lt;script&gt;"));
    assert!(rendered.contains("A &lt;b&gt;bold&lt;/b&gt; claim"));
}

/// The counterpart of `the_download_button_escapes_the_version_it_names`.
#[test]
fn the_index_escapes_a_version_that_carries_markup() {
    let rendered = index_of(vec![entry(
        ReleaseChannel::Stable,
        "1.0.0-beta.0+a<b",
        1,
        SECONDS,
    )])
    .html();
    assert!(!rendered.contains("a<b"), "the version reached the markup");
    assert!(rendered.contains("a&lt;b"));
}

/// Stable has exactly one release and Internal is never published. A channel
/// with nothing in it still gets a tab and a panel, so the page never looks
/// broken on a fresh origin.
#[test]
fn every_public_channel_has_a_tab_and_a_panel_even_when_it_is_empty() {
    let rendered = ReleaseIndex::default().html();
    for channel in ReleaseChannel::PUBLIC {
        assert!(
            rendered.contains(&format!("id=\"t-{channel}\"")),
            "{channel} tab"
        );
        assert!(
            rendered.contains(&format!("id=\"p-{channel}\"")),
            "{channel} panel"
        );
        assert!(rendered.contains(&format!("No {channel} releases yet.")));
    }
    assert!(
        !rendered.contains("t-internal"),
        "internal is never published and has no tab"
    );
}

/// **The page must work with JavaScript off**, so every panel's content is in
/// the markup and the only script is the deep-link helper. Nothing is fetched
/// and nothing is hidden behind an attribute.
#[test]
fn every_channel_reaches_its_releases_without_javascript() {
    let index = index_of(vec![
        entry(ReleaseChannel::Stable, "0.0.1", 1, NANOS),
        entry(ReleaseChannel::Beta, "0.0.2-beta.1", 3, SECONDS),
        entry(
            ReleaseChannel::Nightly,
            "0.0.3-nightly.20260821+9656909",
            33,
            SECONDS,
        ),
    ]);
    let rendered = index.html();

    for row in &index.entries {
        assert!(
            rendered.contains(&row.notes_href().expect("a link")),
            "{} is not reachable in the markup",
            row.version
        );
        assert!(rendered.contains(&row.title));
    }
    assert_eq!(
        rendered.matches("<script").count(),
        1,
        "the only script is the hash helper"
    );
    assert!(
        !rendered.contains("<details"),
        "switching is not disclosure"
    );
    assert!(
        !rendered.contains("hidden"),
        "no panel is hidden by an attribute"
    );
    // The radios must precede the nav and the panels, or `~` cannot reach them.
    let first_input = rendered.find("class=\"pick\"").expect("a radio");
    assert!(first_input < rendered.find("class=\"tabs\"").expect("the nav"));
    assert!(first_input < rendered.find("class=\"panel\"").expect("a panel"));
}

/// A schema bump has to be a refusal rather than a silent misread — the index
/// is rewritten wholesale, so a misparse would publish a page that drops
/// releases.
#[test]
fn a_newer_index_is_refused_rather_than_misread() {
    let round_trip = index_of(vec![entry(ReleaseChannel::Stable, "0.0.1", 1, NANOS)]);
    let bytes = round_trip.to_json().unwrap();
    assert_eq!(
        ReleaseIndex::parse(&bytes).unwrap().entries,
        round_trip.entries
    );

    let newer = String::from_utf8(bytes)
        .unwrap()
        .replace("\"schema_version\": 1", "\"schema_version\": 2");
    assert!(ReleaseIndex::parse(newer.as_bytes()).is_err());
}

/// The projection keeps what the page draws and drops what it does not.
#[test]
fn an_entry_is_a_projection_of_the_manifest_it_was_published_from() {
    let manifest = UpdateManifest {
        schema_version: 1,
        channel: ReleaseChannel::Stable,
        platform: "windows".into(),
        architecture: "x86_64".into(),
        version: "0.0.1".into(),
        build_number: 1,
        published_at: NANOS.into(),
        commit: Some("abc1234".into()),
        source: None,
        minimum_updater_version: Some(1),
        minimum_supported_build: None,
        required: false,
        release_notes: ReleaseNotes {
            title: "First cloud release".into(),
            summary: vec!["Documents show their chrome only when you need it.".into()],
            url: Some(format!("https://{HOST}/releases/0.0.1/index.html")),
        },
        package: UpdatePackage {
            url: format!("https://{HOST}/releases/0.0.1/koru-{WINDOWS}.zip"),
            file_name: format!("koru-{WINDOWS}.zip"),
            size: 45_088_768,
            sha256: "a".repeat(64),
            signature: "untrusted comment: ...".into(),
            format: PackageFormat::Zip,
        },
    };

    // The channel comes from the key it was found under, not the body: this is
    // how the beta pointer keeps its own row.
    let promoted = ReleaseIndexEntry::from_manifest(&manifest, ReleaseChannel::Beta);
    assert_eq!(promoted.channel, ReleaseChannel::Beta);
    assert_eq!(promoted.version, "0.0.1");
    assert!(promoted.promoted_from_stable());
    assert_eq!(promoted.title, "First cloud release");
    assert_eq!(promoted.size, 45_088_768);
}

/// Write a real page out so it can be opened in a browser, because the
/// assertions above prove the markup is *present* and prove nothing about
/// whether it reads well — or whether the tabs actually switch.
/// `KORU_RELEASES_PREVIEW=<path> cargo test -p koru-release preview` — off by
/// default, so an ordinary run writes nothing. The same idiom as
/// `preview_the_notes_page`.
#[test]
fn preview_the_release_index() {
    let Ok(destination) = std::env::var("KORU_RELEASES_PREVIEW") else {
        return;
    };
    let mut entries = vec![
        entry(ReleaseChannel::Stable, "0.0.1", 1, NANOS),
        entry(ReleaseChannel::Beta, "0.0.1", 1, NANOS),
        entry(
            ReleaseChannel::Beta,
            "0.0.2-beta.0",
            2,
            "2026-08-05T09:00:00Z",
        ),
        entry(
            ReleaseChannel::Beta,
            "0.0.2-beta.1",
            3,
            "2026-08-05T11:00:00Z",
        ),
    ];
    for (offset, commit) in ["3a4b308", "5d3f262", "9656909"].iter().enumerate() {
        let mut nightly = entry(
            ReleaseChannel::Nightly,
            &format!("0.0.3-nightly.2026082{offset}+{commit}"),
            (offset as u64) + 4,
            SECONDS,
        );
        nightly.title = "Pin the pages you keep coming back to".into();
        nightly.summary = vec![
            "Right-click a document tab to pin it.".into(),
            "Installing an update now shows its progress.".into(),
        ];
        entries.push(nightly);
    }
    std::fs::write(&destination, index_of(entries).html()).expect("write the preview");
    println!("wrote {destination}");
}

fn manifest(channel: ReleaseChannel, target: &str, version: &str, build: u64) -> UpdateManifest {
    let (platform, architecture) = target.split_once('-').expect("a target");
    UpdateManifest {
        schema_version: 1,
        channel,
        platform: platform.into(),
        architecture: architecture.into(),
        version: version.into(),
        build_number: build,
        published_at: SECONDS.into(),
        commit: Some("abc1234".into()),
        source: None,
        minimum_updater_version: Some(1),
        minimum_supported_build: None,
        required: false,
        release_notes: ReleaseNotes {
            title: format!("Release {version}"),
            summary: vec!["Something changed.".into()],
            url: Some(format!("https://{HOST}/releases/{version}/index.html")),
        },
        package: UpdatePackage {
            url: format!("https://{HOST}/releases/{version}/koru-{target}.zip"),
            file_name: format!("koru-{target}.zip"),
            size: 1,
            sha256: "a".repeat(64),
            signature: "untrusted comment: ...".into(),
            format: PackageFormat::Zip,
        },
    }
}

fn floors_of(pairs: &[(&str, u64)]) -> IndexFloors {
    IndexFloors {
        schema_version: FLOOR_SCHEMA_VERSION,
        floors: pairs
            .iter()
            .map(|(pair, build)| ((*pair).to_owned(), *build))
            .collect(),
    }
}

fn builds(index: &ReleaseIndex) -> Vec<(ReleaseChannel, String, u64)> {
    let mut rows: Vec<_> = index
        .entries
        .iter()
        .map(|entry| (entry.channel, entry.target.clone(), entry.build_number))
        .collect();
    rows.sort_by_key(|(channel, target, build)| (channel.as_str(), target.clone(), *build));
    rows
}

/// What the live origin looks like: Windows and macOS nightlies interleaved on
/// one counter, the Mac several numbers behind, and a stable build that beta
/// also served.
fn a_history() -> ReleaseIndex {
    index_of(vec![
        entry(ReleaseChannel::Stable, "0.0.1", 1, NANOS),
        entry(ReleaseChannel::Beta, "0.0.1", 1, NANOS),
        entry(ReleaseChannel::Beta, "0.0.2-beta.0", 2, SECONDS),
        entry_for(
            WINDOWS,
            ReleaseChannel::Nightly,
            "0.0.3-nightly.a",
            40,
            SECONDS,
        ),
        entry_for(
            MACOS,
            ReleaseChannel::Nightly,
            "0.0.3-nightly.a",
            41,
            SECONDS,
        ),
        entry_for(
            WINDOWS,
            ReleaseChannel::Nightly,
            "0.0.3-nightly.b",
            50,
            SECONDS,
        ),
        entry_for(
            MACOS,
            ReleaseChannel::Nightly,
            "0.0.3-nightly.b",
            51,
            SECONDS,
        ),
        entry_for(
            WINDOWS,
            ReleaseChannel::Nightly,
            "0.0.3-nightly.c",
            60,
            SECONDS,
        ),
    ])
}

/// A stand-in origin serving `history` under the keys `publish_manifest` uses.
fn serve(history: &[UpdateManifest], key: &str) -> Option<Vec<u8>> {
    history
        .iter()
        .find(|manifest| {
            key == format!(
                "history/{}/{}-{}/{}.json",
                manifest.channel, manifest.platform, manifest.architecture, manifest.build_number
            )
        })
        .map(|manifest| serde_json::to_vec(manifest).expect("a manifest serializes"))
}

/// **The reason the floor is per target.** The newest Windows nightly is 60
/// and the newest Mac one 51; a single floor at 60 would hide every Mac.
#[test]
fn keep_latest_keeps_the_newest_build_of_every_channel_and_target() {
    let (trimmed, floors, hidden) = trim_to_latest(a_history(), IndexFloors::default());

    assert_eq!(
        floors,
        floors_of(&[
            ("beta/windows-x86_64", 2),
            ("nightly/macos-aarch64", 51),
            ("nightly/windows-x86_64", 60),
            ("stable/windows-x86_64", 1),
        ])
    );
    assert_eq!(
        builds(&trimmed),
        vec![
            (ReleaseChannel::Beta, WINDOWS.into(), 2),
            (ReleaseChannel::Nightly, MACOS.into(), 51),
            (ReleaseChannel::Nightly, WINDOWS.into(), 60),
            (ReleaseChannel::Stable, WINDOWS.into(), 1),
        ],
        "mac and windows each keep their own latest"
    );
    assert_eq!(hidden, 4);
}

/// Running it again changes nothing, and a floor already above what the index
/// carries is never lowered — so it cannot bring back what was hidden.
#[test]
fn keep_latest_is_idempotent_and_never_lowers_a_floor() {
    let (once, floors, _) = trim_to_latest(a_history(), IndexFloors::default());
    let (twice, again, hidden) = trim_to_latest(once.clone(), floors.clone());
    assert_eq!(again, floors);
    assert_eq!(twice.entries, once.entries);
    assert_eq!(hidden, 0);

    let (_, kept, _) = trim_to_latest(a_history(), floors_of(&[("nightly/macos-aarch64", 99)]));
    assert_eq!(kept.floor(ReleaseChannel::Nightly, MACOS), 99);
}

/// The absent file is no floor, and filtering by it changes nothing.
#[test]
fn a_missing_floor_file_changes_nothing() {
    let mut index = a_history();
    assert_eq!(index.apply_floors(&IndexFloors::default()), 0);
    assert_eq!(index.entries, a_history().entries);

    let (json, html) = rendered(a_history(), &IndexFloors::default()).unwrap();
    assert_eq!(
        ReleaseIndex::parse(&json).unwrap().entries,
        a_history().entries
    );
    assert_eq!(html, a_history().html());
}

/// `store` uploads what `rendered` returns, so a stale index carrying rows
/// below the floor never reaches the origin — in the JSON or on the page.
#[test]
fn the_stored_index_and_page_leave_out_what_is_below_a_floor() {
    let floors = floors_of(&[
        ("nightly/windows-x86_64", 60),
        ("nightly/macos-aarch64", 51),
    ]);
    let (json, html) = rendered(a_history(), &floors).unwrap();

    let stored = ReleaseIndex::parse(&json).unwrap();
    assert!(stored.entries.iter().all(|entry| floors.admits(entry)));
    assert_eq!(stored.by_channel(ReleaseChannel::Nightly).len(), 2);
    assert!(html.contains("0.0.3-nightly.c"));
    assert!(
        html.contains("0.0.3-nightly.b"),
        "the mac build at its floor"
    );
    assert!(!html.contains("0.0.3-nightly.a"), "{html}");
    assert_eq!(
        stored.by_channel(ReleaseChannel::Beta).len(),
        2,
        "a channel with no floor is listed in full"
    );
}

/// A later publish is above the floor and is listed as usual, beside the build
/// that was the latest when the floor was set.
#[test]
fn a_publish_above_the_floor_is_listed() {
    let (trimmed, floors, _) = trim_to_latest(a_history(), IndexFloors::default());
    let published = [manifest(
        ReleaseChannel::Nightly,
        MACOS,
        "0.0.3-nightly.d",
        62,
    )];

    let (json, _) = rendered(with_published(trimmed, &published), &floors).unwrap();
    let stored = ReleaseIndex::parse(&json).unwrap();
    let nightly: Vec<_> = stored
        .by_channel(ReleaseChannel::Nightly)
        .iter()
        .map(|entry| (entry.target.clone(), entry.build_number))
        .collect();
    assert_eq!(
        nightly,
        vec![(MACOS.into(), 62), (WINDOWS.into(), 60), (MACOS.into(), 51)]
    );
}

/// **A publish onto a stale index does not resurrect what is hidden.** The
/// index `update` fetches may predate the floor; `store` filters it.
#[test]
fn a_publish_onto_an_index_written_before_the_floor_drops_the_hidden_rows() {
    let floors = floors_of(&[("nightly/windows-x86_64", 60)]);
    let published = [manifest(
        ReleaseChannel::Nightly,
        WINDOWS,
        "0.0.3-nightly.d",
        62,
    )];
    let (json, _) = rendered(with_published(a_history(), &published), &floors).unwrap();
    let windows: Vec<u64> = ReleaseIndex::parse(&json)
        .unwrap()
        .entries
        .iter()
        .filter(|entry| entry.channel == ReleaseChannel::Nightly && entry.target == WINDOWS)
        .map(|entry| entry.build_number)
        .collect();
    assert_eq!(windows, vec![60, 62]);
}

/// **A rebuild does not resurrect what is hidden**, and does not even fetch it:
/// every `history/` object below its floor is skipped by key.
#[test]
fn a_rebuild_neither_fetches_nor_lists_what_is_below_a_floor() {
    let history = [
        manifest(ReleaseChannel::Nightly, WINDOWS, "0.0.3-nightly.a", 40),
        manifest(ReleaseChannel::Nightly, MACOS, "0.0.3-nightly.a", 41),
        manifest(ReleaseChannel::Nightly, WINDOWS, "0.0.3-nightly.b", 50),
        manifest(ReleaseChannel::Nightly, MACOS, "0.0.3-nightly.b", 51),
        manifest(ReleaseChannel::Nightly, WINDOWS, "0.0.3-nightly.c", 60),
    ];
    let floors = floors_of(&[
        ("nightly/windows-x86_64", 60),
        ("nightly/macos-aarch64", 51),
    ]);
    let mut asked = Vec::new();
    let rebuilt = rebuild_from(60, &floors, |key| {
        asked.push(key.to_owned());
        Ok(serve(&history, key))
    })
    .unwrap();

    assert_eq!(
        builds(&rebuilt),
        vec![
            (ReleaseChannel::Nightly, MACOS.into(), 51),
            (ReleaseChannel::Nightly, WINDOWS.into(), 60),
        ]
    );
    assert!(
        !asked
            .iter()
            .any(|key| key == "history/nightly/windows-x86_64/50.json")
    );
    assert!(
        !asked
            .iter()
            .any(|key| key == "history/nightly/macos-aarch64/41.json")
    );
    assert!(
        asked
            .iter()
            .any(|key| key == "history/stable/windows-x86_64/1.json"),
        "a channel with no floor is still probed from build 1"
    );

    let whole = rebuild_from(60, &IndexFloors::default(), |key| Ok(serve(&history, key))).unwrap();
    assert_eq!(whole.entries.len(), 5, "no floor, the whole history");
}

/// A floor file this build does not understand is refused rather than read as
/// "no floor", which would republish everything it hides.
#[test]
fn a_floor_file_round_trips_and_a_newer_one_is_refused() {
    let floors = floors_of(&[("nightly/macos-aarch64", 51)]);
    let bytes = floors.to_json().unwrap();
    assert_eq!(IndexFloors::parse(&bytes).unwrap(), floors);
    let newer = String::from_utf8(bytes)
        .unwrap()
        .replace("\"schema_version\": 1", "\"schema_version\": 2");
    assert!(IndexFloors::parse(newer.as_bytes()).is_err());
    assert!(IndexFloors::parse(b"not json").is_err());
}
