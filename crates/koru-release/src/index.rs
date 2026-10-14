// SPDX-License-Identifier: MPL-2.0

//! The release index — what `https://updates.koru.do/releases/` shows.
//!
//! Koru publishes a page per release and a channel manifest naming only the
//! newest build. Nothing listed what had shipped, which matters more than it
//! sounds: [ADR-113] took the channel switcher out of the app, so downloading
//! is the only way to move between channels, and a person on nightly who wants
//! stable had no page to go to.
//!
//! # Where the truth is
//!
//! **The origin, not the repository.** `releases/*.toml` records what somebody
//! *wrote*; `history/<channel>/<target>/<build>.json` records what was
//! *published*, and the two have already diverged —
//! `0.0.3-nightly.20260816+edb367e` shipped with no descriptor in the tree,
//! while two descriptors name releases that were never published. An index
//! built from the tree would advertise two downloads that 404 and hide one that
//! exists.
//!
//! # Why this probes instead of listing
//!
//! There is no list-objects call available. Cloudflare's own documentation:
//! *"The Object Read & Write and Object Read only permissions are only
//! supported by the S3-compatible API, not the Cloudflare REST API."* Listing
//! would mean minting S3 access keys — a second long-lived credential to store
//! and back up, for a tool run by hand a few times a year. So `rebuild` walks
//! build numbers instead, and a 404 means *that number never became a release*
//! rather than *stop*. Ten numbers are holes today (5, 6, 11, 24, 26–28,
//! 30–32): `allocate_build_number` writes the counter before the build starts,
//! so every abandoned `dist-cloud` burns one permanently.

use crate::cloud::{Cloud, HOST};
use crate::page::{self, escape_html};
use crate::{MANIFEST_CACHE, PUBLISHED_TARGETS, validate_release_version};
use chrono::DateTime;
use koru_update::{ReleaseChannel, UpdateManifest};
use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;

/// Derived data, and deliberately **not** under `/releases/`: everything with
/// that prefix carries a one-year edge and browser TTL, and this object is
/// rewritten on every publish. `_publisher/` already holds `build-number.json`
/// under the same reasoning.
///
/// It is publicly readable, like everything else in the bucket — `_publisher/`
/// is a naming convention, not access control. That is fine, since every field
/// is a projection of an already-public manifest, but do not put anything in
/// that prefix on the assumption it is private.
pub const INDEX_KEY: &str = "_publisher/releases.json";

/// The one object under `/releases/` that is deliberately overwritten. The
/// collision check in `publish` covers the package and the notes page and must
/// never be extended to cover this.
pub const PAGE_KEY: &str = "releases/index.html";

/// Where the index starts, per channel and target — see [`IndexFloors`].
///
/// Under `_publisher/` for the same reason as [`INDEX_KEY`]: it is rewritten
/// by hand, and anything under `/releases/` would be frozen at the edge for a
/// year.
pub const FLOOR_KEY: &str = "_publisher/index-floor.json";

const SCHEMA_VERSION: u32 = 1;
const FLOOR_SCHEMA_VERSION: u32 = 1;

/// One published release, as the page needs it.
///
/// A projection of `UpdateManifest` rather than the whole thing: the signature
/// is 200 bytes that say nothing to a reader, and this file is fetched and
/// rewritten on every publish.
#[derive(Clone, Debug, Deserialize, Serialize, PartialEq, Eq)]
pub struct ReleaseIndexEntry {
    /// The channel this build was **served on**, taken from the key prefix it
    /// was found under rather than from the manifest body. A stable release is
    /// republished under `beta` as a pointer, so one build can legitimately
    /// appear on two channels; see `promoted_from_stable`.
    pub channel: ReleaseChannel,
    pub version: String,
    pub build_number: u64,
    /// Verbatim from the manifest. **Two formats are live in the bucket** —
    /// build 1 carries `2026-08-05T04:54:53.664004300+00:00`, written before
    /// the seconds-precision rule landed, and build 33 carries
    /// `2026-08-21T13:48:54Z`. Rendered through `date`, never compared.
    pub published_at: String,
    #[serde(default)]
    pub commit: Option<String>,
    pub title: String,
    pub summary: Vec<String>,
    /// Verbatim from the manifest, where it is optional.
    #[serde(default)]
    pub notes_url: Option<String>,
    pub package_url: String,
    pub size: u64,
    /// `windows-x86_64`, `macos-aarch64`. **A build number belongs to one
    /// target**: each publish allocates a fresh one, so a version shipped for
    /// two platforms is two builds and two rows. Without this they are two rows
    /// with the same title, the same version and no way to tell which download
    /// is which. `default` because entries written before macOS existed carry
    /// no target, and every one of those is Windows.
    #[serde(default = "windows_target")]
    pub target: String,
}

/// The only target that had been published when the index was first written.
fn windows_target() -> String {
    "windows-x86_64".to_owned()
}

impl ReleaseIndexEntry {
    pub fn from_manifest(manifest: &UpdateManifest, channel: ReleaseChannel) -> Self {
        Self {
            channel,
            version: manifest.version.clone(),
            build_number: manifest.build_number,
            published_at: manifest.published_at.clone(),
            commit: manifest.commit.clone(),
            title: manifest.release_notes.title.clone(),
            summary: manifest.release_notes.summary.clone(),
            notes_url: manifest.release_notes.url.clone(),
            package_url: manifest.package.url.clone(),
            size: manifest.package.size,
            target: format!("{}-{}", manifest.platform, manifest.architecture),
        }
    }

    /// What to call `target` on the page.
    ///
    /// A version shipped for two platforms is two builds and two rows with the
    /// same title, so the row has to say which download it is or the second
    /// reads as a duplicate. An unrecognised target renders **verbatim** rather
    /// than being dropped or guessed at: this page is built from what the
    /// origin already serves, and a target added tomorrow must appear as itself
    /// rather than vanish until this table catches up.
    fn platform_name(&self) -> &str {
        match self.target.as_str() {
            "windows-x86_64" => "Windows",
            "macos-aarch64" => "macOS (Apple silicon)",
            "macos-x86_64" => "macOS (Intel)",
            other => other,
        }
    }

    /// `2026-08-21`, in UTC.
    ///
    /// Falls back to the first ten characters when the timestamp will not
    /// parse. A release that has already shipped must still appear in the list:
    /// a page that refuses to render because one of two dozen dates is odd is
    /// worse than a page with one ugly date on it.
    fn date(&self) -> String {
        DateTime::parse_from_rfc3339(&self.published_at)
            .map(|stamp| stamp.naive_utc().format("%Y-%m-%d").to_string())
            .unwrap_or_else(|_| self.published_at.chars().take(10).collect())
    }

    /// `/releases/<version>/index.html` — **root-absolute, not sibling-relative.**
    ///
    /// The opposite choice from `Download::html`, and for a reason worth
    /// keeping: the notes page and its zip are siblings inside one immutable
    /// prefix, so a bare file name there survives the origin moving. This page
    /// links *across* prefixes and is itself served at two paths (`/releases/`
    /// and `/releases/index.html`), so a relative href would resolve
    /// differently depending on which one the reader arrived at.
    ///
    /// A URL naming another origin is passed through untouched, and `None`
    /// stays `None` — the caller renders the title unlinked rather than
    /// inventing a destination.
    fn notes_href(&self) -> Option<String> {
        let url = self.notes_url.as_deref()?;
        Some(match url.strip_prefix(&format!("https://{HOST}")) {
            Some(path) => path.to_string(),
            None => url.to_string(),
        })
    }

    /// A beta entry whose version is not a beta version: the stable pointer.
    ///
    /// Judged by the rule that already decides what a native beta version looks
    /// like on the way in, so there is one definition rather than a second
    /// string test that can disagree with it.
    fn promoted_from_stable(&self) -> bool {
        self.channel == ReleaseChannel::Beta
            && validate_release_version(ReleaseChannel::Beta, &self.version).is_err()
    }
}

#[derive(Clone, Debug, Deserialize, Serialize)]
pub struct ReleaseIndex {
    pub schema_version: u32,
    pub entries: Vec<ReleaseIndexEntry>,
}

impl Default for ReleaseIndex {
    fn default() -> Self {
        Self {
            schema_version: SCHEMA_VERSION,
            entries: Vec::new(),
        }
    }
}

impl ReleaseIndex {
    pub fn parse(bytes: &[u8]) -> Result<Self, String> {
        let index: Self = serde_json::from_slice(bytes)
            .map_err(|error| format!("the published release index is invalid: {error}"))?;
        if index.schema_version != SCHEMA_VERSION {
            return Err(format!(
                "the published release index is schema {} and this build writes {SCHEMA_VERSION}",
                index.schema_version
            ));
        }
        Ok(index)
    }

    pub fn to_json(&self) -> Result<Vec<u8>, String> {
        serde_json::to_vec_pretty(self).map_err(|error| error.to_string())
    }

    /// Replace on `(channel, build_number)`, else append.
    ///
    /// The key is the pair and not the version, because a stable release really
    /// is served on two channels and both rows are true.
    pub fn upsert(&mut self, entry: ReleaseIndexEntry) {
        match self
            .entries
            .iter_mut()
            .find(|held| held.channel == entry.channel && held.build_number == entry.build_number)
        {
            Some(held) => *held = entry,
            None => self.entries.push(entry),
        }
    }

    /// Drop every entry below its `channel/target` floor, and return how many
    /// went.
    ///
    /// **The only place a floor is applied.** `update`, `rebuild` and `store`
    /// all call it, so a publish, a repair and an upload cannot disagree about
    /// what is listed.
    pub fn apply_floors(&mut self, floors: &IndexFloors) -> usize {
        let before = self.entries.len();
        self.entries.retain(|entry| floors.admits(entry));
        before - self.entries.len()
    }

    /// Newest first, **by build number rather than by date**.
    ///
    /// `published_at` is one `Utc::now()` read *before* the build starts, so a
    /// long build can stamp an older release later than a newer one, and two
    /// publications started in the same second tie. Build numbers come from one
    /// monotonic counter, are allocated once, and cannot tie within a channel.
    /// Sorting the strings would also be wrong across the two live timestamp
    /// formats: `.664004300+00:00` sorts before `Z` because `.` is below `Z`,
    /// while 53.664 is after 53.
    fn by_channel(&self, channel: ReleaseChannel) -> Vec<&ReleaseIndexEntry> {
        let mut rows: Vec<_> = self
            .entries
            .iter()
            .filter(|entry| entry.channel == channel)
            .collect();
        rows.sort_by_key(|entry| std::cmp::Reverse(entry.build_number));
        rows
    }

    pub fn html(&self) -> String {
        let mut body = String::from(
            "<a href=\"https://koru.do\">Koru</a><h1>Releases</h1>\
             <p class=\"lede\">Koru releases, newest first. Channels install side by side \
             and share nothing you have written, so moving between them costs no content.</p>",
        );

        // Every input precedes the nav and the panels, because the sibling
        // combinator only reaches forwards.
        for (position, channel) in ReleaseChannel::PUBLIC.iter().enumerate() {
            body.push_str(&format!(
                "<input type=\"radio\" name=\"ch\" id=\"t-{channel}\" class=\"pick\"{checked}>",
                checked = if position == 0 { " checked" } else { "" },
            ));
        }

        body.push_str("<nav class=\"tabs\">");
        for channel in ReleaseChannel::PUBLIC {
            let count = self.by_channel(channel).len();
            body.push_str(&format!(
                "<label for=\"t-{channel}\">{name} <span>{count}</span></label>",
                name = channel_name(channel),
            ));
        }
        body.push_str("</nav>");

        for channel in ReleaseChannel::PUBLIC {
            body.push_str(&self.panel(channel));
        }

        page::shell(
            "Koru releases",
            TAB_STYLE,
            // `/releases/` and `/releases/index.html` are the same page; without
            // this they compete as duplicates.
            "<link rel=\"canonical\" href=\"https://updates.koru.do/releases/\">",
            &format!("{body}{HASH_SCRIPT}"),
        )
    }

    fn panel(&self, channel: ReleaseChannel) -> String {
        let rows = self.by_channel(channel);
        let mut panel = format!(
            "<section class=\"panel\" id=\"p-{channel}\"><h2>{name} releases</h2>",
            name = channel_name(channel),
        );
        if channel == ReleaseChannel::Beta {
            panel.push_str(
                "<p class=\"lede\">Beta receives every stable release as well as its own.</p>",
            );
        }
        if rows.is_empty() {
            panel.push_str(&format!(
                "<p class=\"lede\">No {} releases yet.</p></section>",
                channel_name(channel).to_lowercase()
            ));
            return panel;
        }
        for entry in rows {
            panel.push_str(&self.row(entry));
        }
        panel.push_str("</section>");
        panel
    }

    fn row(&self, entry: &ReleaseIndexEntry) -> String {
        let title = escape_html(&entry.title);
        let heading = match entry.notes_href() {
            Some(href) => format!("<a href=\"{}\">{title}</a>", escape_html(&href)),
            None => title,
        };
        let promoted = if entry.promoted_from_stable() {
            " <span class=\"tag\">also released on stable</span>"
        } else {
            ""
        };
        let summary = entry
            .summary
            .iter()
            .map(|line| format!("<li>{}</li>", escape_html(line)))
            .collect::<String>();
        format!(
            "<article><h3>{heading}</h3>\
             <p class=\"meta\">{version} · {platform} · <time datetime=\"{stamp}\">{date}</time> · \
             build {build}{promoted}</p><ul>{summary}</ul></article>",
            version = escape_html(&entry.version),
            platform = escape_html(entry.platform_name()),
            stamp = escape_html(&entry.published_at),
            date = escape_html(&entry.date()),
            build = entry.build_number,
        )
    }
}

/// **The oldest build the index lists, per `channel/target`.** A build below
/// its floor stays on the origin — its package, its notes page and its
/// `history/` object are untouched, and an installed client never reads this
/// file — but it is not listed on `/releases/`, in `releases.json`, or on the
/// website that reads it.
///
/// Per channel *and target* rather than one global number because a build
/// number belongs to one target and the platforms are published separately:
/// the newest macOS build is routinely several numbers below the newest
/// Windows one, so a single floor at the Windows build would hide every Mac.
///
/// **Persisted, because the index is derived.** `rebuild` re-probes every
/// `history/` object, so trimming `releases.json` alone would last only until
/// the next repair. The floor is what the rebuild consults, and
/// [`ReleaseIndex::apply_floors`] is the one place it is applied.
///
/// A missing file is no floor at all, which is what every origin had before
/// `release-index --keep-latest` existed.
#[derive(Clone, Debug, Deserialize, Serialize, PartialEq, Eq)]
pub struct IndexFloors {
    pub schema_version: u32,
    /// `"nightly/macos-aarch64" -> 66`. A `BTreeMap` so the file is written in
    /// one order and a re-run that changes nothing changes no bytes.
    pub floors: BTreeMap<String, u64>,
}

impl Default for IndexFloors {
    fn default() -> Self {
        Self {
            schema_version: FLOOR_SCHEMA_VERSION,
            floors: BTreeMap::new(),
        }
    }
}

impl IndexFloors {
    pub fn parse(bytes: &[u8]) -> Result<Self, String> {
        let floors: Self = serde_json::from_slice(bytes)
            .map_err(|error| format!("the published index floor is invalid: {error}"))?;
        if floors.schema_version != FLOOR_SCHEMA_VERSION {
            return Err(format!(
                "the published index floor is schema {} and this build reads {FLOOR_SCHEMA_VERSION}",
                floors.schema_version
            ));
        }
        Ok(floors)
    }

    pub fn to_json(&self) -> Result<Vec<u8>, String> {
        serde_json::to_vec_pretty(self).map_err(|error| error.to_string())
    }

    fn key(channel: ReleaseChannel, target: &str) -> String {
        format!("{channel}/{target}")
    }

    /// 0 when nothing is set, which admits every build.
    pub fn floor(&self, channel: ReleaseChannel, target: &str) -> u64 {
        self.floors
            .get(&Self::key(channel, target))
            .copied()
            .unwrap_or(0)
    }

    pub fn admits(&self, entry: &ReleaseIndexEntry) -> bool {
        entry.build_number >= self.floor(entry.channel, &entry.target)
    }

    /// Raise every `channel/target` present in `index` to its newest build, so
    /// that only that build stays listed. Floors for pairs the index does not
    /// carry are kept, and no floor is ever lowered: running this twice is
    /// running it once, and it cannot bring back something hidden earlier.
    pub fn keep_latest(&mut self, index: &ReleaseIndex) {
        for entry in &index.entries {
            let floor = self
                .floors
                .entry(Self::key(entry.channel, &entry.target))
                .or_insert(0);
            *floor = (*floor).max(entry.build_number);
        }
    }
}

/// Title case for a label. `ReleaseChannel::as_str` is the wire spelling and
/// stays lowercase; this is the only place a channel is a word a person reads.
fn channel_name(channel: ReleaseChannel) -> &'static str {
    match channel {
        ReleaseChannel::Stable => "Stable",
        ReleaseChannel::Beta => "Beta",
        ReleaseChannel::Nightly => "Nightly",
        ReleaseChannel::Internal => "Internal",
    }
}

/// **Tabs with no JavaScript.** Three hidden radios ahead of the nav and the
/// panels, switched by `:checked ~` sibling selectors.
///
/// Chosen over the alternatives for concrete reasons. `:target` gives shareable
/// URLs but scrolls the chosen panel to the top of the viewport — throwing the
/// heading and the tabs off screen on every click — and needs `:has()` to style
/// the "no fragment yet" default. `<details>` is disclosure, not switching. A
/// JavaScript toggle needs a no-script story, and this needs none.
///
/// The radios are `opacity:0` rather than `display:none`, which keeps them
/// focusable: Tab reaches the group and the arrow keys move between channels,
/// which is the keyboard behaviour a tab strip is supposed to have, for free.
///
/// With CSS off the page degrades to three radio buttons and three
/// `<h2>`-headed sections in reading order — long, but complete. Nothing is
/// hidden behind an attribute.
const TAB_STYLE: &str = ".lede{color:#666;font-size:.95em}\
     .pick{position:absolute;opacity:0;pointer-events:none}\
     .tabs{display:flex;gap:.25rem;border-bottom:1px solid #e0ddd8;margin:2.5rem 0 1.5rem}\
     .tabs label{padding:.5rem .9rem;cursor:pointer;color:#666;border-bottom:2px solid transparent;margin-bottom:-1px}\
     .tabs label span{font-size:.8em;color:#666}\
     .panel{display:none}\
     .panel h2{font-size:1.1em;text-transform:uppercase;letter-spacing:.06em;color:#666;font-weight:600}\
     article{padding:1.25rem 0;border-top:1px solid #e0ddd8}\
     article h3{margin:0 0 .25rem;font-size:1.15em;line-height:1.3}\
     article h3 a{color:#20201f;text-decoration:none}\
     article h3 a:hover{text-decoration:underline}\
     article ul{margin:.6rem 0 0;padding-left:1.1rem}\
     article li{font-size:.92em;margin:.2rem 0}\
     .meta{margin:0;font-size:.82em;color:#666;font-family:ui-monospace,SFMono-Regular,Menlo,monospace}\
     .tag{font-family:system-ui,sans-serif;background:#faf9f7;border:1px solid #e0ddd8;border-radius:99px;padding:.05rem .5rem}\
     #t-stable:checked~.tabs label[for=\"t-stable\"],\
     #t-beta:checked~.tabs label[for=\"t-beta\"],\
     #t-nightly:checked~.tabs label[for=\"t-nightly\"]{color:#20201f;font-weight:600;border-bottom-color:#20201f}\
     #t-stable:checked~.tabs label[for=\"t-stable\"] span,\
     #t-beta:checked~.tabs label[for=\"t-beta\"] span,\
     #t-nightly:checked~.tabs label[for=\"t-nightly\"] span{color:#20201f}\
     #t-stable:checked~#p-stable,\
     #t-beta:checked~#p-beta,\
     #t-nightly:checked~#p-nightly{display:block}\
     #t-stable:focus-visible~.tabs label[for=\"t-stable\"],\n     #t-beta:focus-visible~.tabs label[for=\"t-beta\"],\n     #t-nightly:focus-visible~.tabs label[for=\"t-nightly\"]{outline:2px solid #20201f;outline-offset:2px}";

/// Purely additive: it restores the one thing the radios gave up against
/// `:target`, a shareable `/releases/#nightly`. With JavaScript off the reader
/// loses the deep link and never any content.
const HASH_SCRIPT: &str = "<script>\
     (function(){var p=['stable','beta','nightly'];\
     function pick(n){var e=document.getElementById('t-'+n);if(e){e.checked=true}}\
     if(p.indexOf(location.hash.slice(1))>-1){pick(location.hash.slice(1))}\
     p.forEach(function(n){var e=document.getElementById('t-'+n);\
     if(e){e.addEventListener('change',function(){history.replaceState(null,'','#'+n)})}});\
     })();</script>";

/// Fetch, apply what this publication wrote, store.
///
/// `published` is every manifest the run actually wrote — two when a stable
/// release also advances the beta pointer, one otherwise.
pub fn update(cloud: &Cloud, published: &[UpdateManifest]) -> Result<(), String> {
    let floors = fetch_floors(cloud)?;
    let index = match cloud.fetch_public(INDEX_KEY)? {
        // A corrupt index is evidence that something else went wrong, and
        // overwriting it destroys that evidence. A rebuild is one command away.
        Some(bytes) => ReleaseIndex::parse(&bytes)?,
        None => {
            // Not an error: the first publish after this landed has no index to
            // read. Appending to an empty one would throw the history away
            // silently, and the publish has already spent minutes building, so
            // the rebuild is worth the minute it costs.
            println!("[Koru] no release index yet; rebuilding it from the published history");
            let highest = published
                .iter()
                .map(|manifest| manifest.build_number)
                .max()
                .unwrap_or(0);
            rebuild(cloud, highest, &floors)?
        }
    };
    store(cloud, with_published(index, published), &floors)
}

/// The index plus the rows a publication wrote. Split from `update` so the
/// publish path can be tested without an origin; `store` applies the floors.
pub fn with_published(mut index: ReleaseIndex, published: &[UpdateManifest]) -> ReleaseIndex {
    for manifest in published {
        index.upsert(ReleaseIndexEntry::from_manifest(manifest, manifest.channel));
    }
    index
}

/// What `release-index --keep-latest` stores, worked out without the network:
/// the index with what the existing floors already hide removed, the floors
/// raised to the newest build of every `channel/target` left, and the index
/// trimmed to those. Returns how many entries of `index` are no longer listed.
pub fn trim_to_latest(
    mut index: ReleaseIndex,
    mut floors: IndexFloors,
) -> (ReleaseIndex, IndexFloors, usize) {
    let before = index.entries.len();
    index.apply_floors(&floors);
    floors.keep_latest(&index);
    index.apply_floors(&floors);
    let hidden = before - index.entries.len();
    (index, floors, hidden)
}

/// The floor is uncached for the same reason as the index: it is rewritten,
/// and a rebuild that read a stale copy would list what it hides.
pub fn store_floors(cloud: &Cloud, floors: &IndexFloors) -> Result<(), String> {
    cloud.put_bytes(FLOOR_KEY, floors.to_json()?, "application/json", "no-store")?;
    println!("[Koru] index floor: https://{HOST}/{FLOOR_KEY}");
    Ok(())
}

/// The published floors, or none when the file is absent.
///
/// A corrupt file is an error rather than "no floor": quietly treating it as
/// absent would republish every release the floor was written to hide.
pub fn fetch_floors(cloud: &Cloud) -> Result<IndexFloors, String> {
    match cloud.fetch_public(FLOOR_KEY)? {
        Some(bytes) => IndexFloors::parse(&bytes),
        None => Ok(IndexFloors::default()),
    }
}

/// Probe `history/<channel>/<target>/<n>.json` for every public channel,
/// every published target, and every build number up to `highest`.
///
/// **Every target, because a build number belongs to exactly one of them.**
/// `allocate_build_number` takes the shared counter and adds one on each
/// publish, so a version shipped for Windows and then for macOS is two builds
/// under two prefixes. Probing one target would have quietly dropped every
/// macOS release from the page — which is the failure ADR-147 exists to
/// prevent, arriving from the other direction.
///
/// A 404 is a burnt build number, not a failure. A parse failure is warned
/// about and skipped: a repair tool that refuses to run because one of two
/// dozen objects is odd is not a repair tool.
///
/// A history object below its `channel/target` floor is not fetched at all:
/// the floor is what keeps a repair from resurrecting what
/// `release-index --keep-latest` hid.
pub fn rebuild(cloud: &Cloud, highest: u64, floors: &IndexFloors) -> Result<ReleaseIndex, String> {
    rebuild_from(highest, floors, |key| fetch_with_one_retry(cloud, key))
}

/// `rebuild` with the fetch passed in, so a test can stand in for the origin.
fn rebuild_from(
    highest: u64,
    floors: &IndexFloors,
    mut fetch: impl FnMut(&str) -> Result<Option<Vec<u8>>, String>,
) -> Result<ReleaseIndex, String> {
    let mut index = ReleaseIndex::default();
    let mut found = 0;
    println!(
        "[Koru] rebuilding the release index from builds 1..={highest} across {} channels and {} targets",
        ReleaseChannel::PUBLIC.len(),
        PUBLISHED_TARGETS.len()
    );
    for build in 1..=highest {
        for channel in ReleaseChannel::PUBLIC {
            for target in PUBLISHED_TARGETS {
                if build < floors.floor(channel, target) {
                    continue;
                }
                let key = format!("history/{channel}/{target}/{build}.json");
                let Some(bytes) = fetch(&key)? else {
                    continue;
                };
                // Plain deserialization, not `parse_and_validate`: this reads what
                // was published, and refusing to list a release that is already
                // live because it fails a validator written afterwards would hide
                // exactly the release somebody came to the page to find.
                match serde_json::from_slice::<UpdateManifest>(&bytes) {
                    Ok(manifest) => {
                        if manifest.channel != channel {
                            println!(
                                "[Koru] warning: {key} says channel {}; using {channel} from the key",
                                manifest.channel
                            );
                        }
                        index.upsert(ReleaseIndexEntry::from_manifest(&manifest, channel));
                        found += 1;
                    }
                    Err(error) => println!("[Koru] warning: skipping {key}: {error}"),
                }
            }
        }
        if build % 10 == 0 {
            println!("[Koru] ...{build}/{highest}, {found} releases so far");
        }
    }
    println!("[Koru] found {found} published releases");
    // The probe skipped what is below a floor by its key; this catches a
    // manifest whose body names a different target from the key it sits under.
    index.apply_floors(floors);
    Ok(index)
}

/// One retry on a non-404 error.
///
/// `fetch_public` returns `Err` on any non-success, and a rebuild is ~100
/// sequential requests through the edge — a single 503 at request 87 would
/// otherwise throw away 86 good probes.
fn fetch_with_one_retry(cloud: &Cloud, key: &str) -> Result<Option<Vec<u8>>, String> {
    match cloud.fetch_public(key) {
        Ok(found) => Ok(found),
        Err(first) => {
            println!("[Koru] warning: {key} failed ({first}); retrying once");
            cloud.fetch_public(key)
        }
    }
}

/// The JSON first, then the page.
///
/// The same ordering rule the publish path follows: never leave a page up that
/// claims more than the record it was rendered from. Takes the index by value
/// and applies `floors` itself, so nothing reaches the origin unfiltered — an
/// index fetched from before the floor was raised loses what it now hides.
pub fn store(cloud: &Cloud, index: ReleaseIndex, floors: &IndexFloors) -> Result<(), String> {
    let (json, html) = rendered(index, floors)?;
    cloud.put_bytes(INDEX_KEY, json, "application/json", "no-store")?;
    cloud.put_bytes(
        PAGE_KEY,
        html.into_bytes(),
        "text/html; charset=utf-8",
        MANIFEST_CACHE,
    )?;
    println!("[Koru] release index: https://{HOST}/releases/");
    warn_if_the_edge_will_freeze_it(cloud);
    Ok(())
}

/// `releases.json` and the page, as `store` uploads them and a dry run writes
/// them: both from one floor-filtered index.
pub fn rendered(
    mut index: ReleaseIndex,
    floors: &IndexFloors,
) -> Result<(Vec<u8>, String), String> {
    index.apply_floors(floors);
    Ok((index.to_json()?, index.html()))
}

/// **The one mistake worth catching immediately rather than in a year.**
///
/// Publishing this page before `cloud-setup` has narrowed the `/releases/`
/// cache rule leaves it with a one-year *browser* TTL, and a browser TTL is not
/// reachable by a purge: every visitor who loaded it once would keep that copy
/// until 2027. Nothing about the upload looks wrong when it happens, because
/// the rule overrides the header the object was uploaded with — so the header
/// has to be read back from the edge.
///
/// A warning rather than an error. The bytes are already up and correct; what
/// is wrong is the zone configuration, and failing here would neither undo the
/// upload nor tell the operator anything this does not.
fn warn_if_the_edge_will_freeze_it(cloud: &Cloud) {
    let Ok(Some(header)) = cloud.edge_cache_control(PAGE_KEY) else {
        // A probe that does not answer is not evidence of anything. The rules
        // are checked properly by `cloud-setup`; this is a courtesy.
        return;
    };
    if header.contains("immutable") || header.contains("max-age=31536000") {
        println!("[Koru] WARNING: the edge is serving the index as `{header}`.");
        println!(
            "[Koru] That is the immutable /releases/ rule, and its browser TTL cannot be purged."
        );
        println!(
            "[Koru] Run `koru-release cloud-setup` to narrow it, then purge {INDEX_PATH_HINT} at the edge."
        );
    }
}

const INDEX_PATH_HINT: &str = "https://updates.koru.do/releases/ and /releases/index.html";

#[cfg(test)]
mod tests;
