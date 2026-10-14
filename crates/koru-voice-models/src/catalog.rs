// SPDX-License-Identifier: MPL-2.0

//! **Which models exist: the signed descriptor, plus the unsigned facts about
//! where to get it.**
//!
//! A [`CatalogEntry`] is two things with different trust, and the Rust struct
//! says so rather than leaving it to a reader. Everything inside
//! [`ModelDescriptor`] is covered by the signature; `url` and `mirrors` are
//! not, deliberately, so a download host can be added or withdrawn without
//! re-signing every model
//! ([ADR-250](../../../docs/decisions/ADR-226-250.md)). The JSON is flat — the
//! descriptor is `#[serde(flatten)]`ed — so an entry on the wire reads exactly
//! as `ROADMAP.rev3`'s Plan 1.1 writes it, while the type keeps the boundary
//! that matters visible in code.
//!
//! ## What a mirror is allowed to do
//!
//! Refuse. That is all. The signed `size` and `digest` constrain everything
//! else it could return, so an entry's URL list is an availability convenience
//! and not a trust decision — which is exactly why it is outside the envelope.
//!
//! ## Forward compatibility, which is not the same as leniency
//!
//! A catalog may list a model whose `schema_version` this build does not know.
//! That entry must not stop the ones it *does* know from being installed, so
//! parsing keeps it and [`Catalog::choose`] skips it. What is refused at parse
//! time is only what is structurally wrong for everybody: malformed JSON, a URL
//! that is not HTTPS, an `id`/`version` listed twice.

use crate::descriptor::{DESCRIPTOR_SCHEMA_VERSION, ModelDescriptor, ModelSignature, ModelVersion};
use crate::error::{ModelError, ModelErrorCode};
use crate::tier::ModelTier;
use crate::trust::TrustedKeys;
use koru_voice::{EngineId, ModelId, Requested};
use serde::{Deserialize, Serialize};
use url::Url;

/// The version of the catalog document itself, which is **not** the descriptor
/// schema version. A catalog can gain a field — a deprecation notice, a
/// publication date — without anything signed changing, and the two numbers
/// move independently for that reason.
pub const CATALOG_SCHEMA_VERSION: u32 = 1;

/// One model, as the catalog lists it.
///
/// The fields Plan 1.1 names are all here; they are grouped by *what the
/// signature covers* rather than listed flat, because that grouping is the
/// security property and a flat struct would make it a comment.
#[derive(Clone, PartialEq, Eq, Debug, Serialize, Deserialize)]
pub struct CatalogEntry {
    /// Everything that is signed. Flattened on the wire, so the JSON carries
    /// `id`, `version`, `engine`, … beside `url` and `signature`.
    #[serde(flatten)]
    pub descriptor: ModelDescriptor,
    /// The signature over [`ModelDescriptor::canonical_bytes`] — over the
    /// descriptor, not merely the archive.
    pub signature: ModelSignature,
    /// Where to fetch the archive. **Outside the envelope.**
    pub url: Url,
    /// Further places the same bytes can be fetched from. Also outside the
    /// envelope, and tried in order after [`CatalogEntry::url`].
    #[serde(default)]
    pub mirrors: Vec<Url>,
}

impl CatalogEntry {
    pub fn id(&self) -> &ModelId {
        &self.descriptor.id
    }

    pub fn version(&self) -> &ModelVersion {
        &self.descriptor.version
    }

    pub fn engine(&self) -> &EngineId {
        &self.descriptor.engine
    }

    pub fn tier(&self) -> ModelTier {
        self.descriptor.tier
    }

    /// Every place these bytes can be fetched from, preferred first.
    ///
    /// A `Vec` rather than an iterator over two fields, because the caller that
    /// wants this is a download loop that needs to try the next one after a
    /// failure, and borrowing across a retry is a fight nobody needs to have.
    pub fn sources(&self) -> Vec<&Url> {
        std::iter::once(&self.url)
            .chain(self.mirrors.iter())
            .collect()
    }

    /// **Is this entry from us?**
    ///
    /// The signature check, and nothing else — the archive is not involved and
    /// need not exist yet. It runs *before* a download for exactly that reason:
    /// hashing 650 MB to discover the descriptor was forged is work nobody
    /// needed to do.
    pub fn verify(&self, keys: &TrustedKeys) -> Result<(), ModelError> {
        keys.verify(&self.descriptor, &self.signature)
    }

    /// **What a consent step would put on the screen**, as the thing a grant is
    /// recorded against.
    ///
    /// Built here rather than at the surface so that the sentence somebody
    /// reads and the record that is kept afterwards cannot disagree about what
    /// was offered. Every field of it is inside the signature envelope — the
    /// size, the licence and the attribution are signed for exactly this
    /// reason ([ADR-250](../../../docs/decisions/ADR-226-250.md)): a licence a
    /// catalog can change is a licence nobody agreed to.
    pub fn terms(&self) -> koru_voice_consent::Terms {
        koru_voice_consent::Terms {
            model: self.descriptor.id.as_str().to_owned(),
            version: self.descriptor.version.as_str().to_owned(),
            licence: self.descriptor.licence.as_str().to_owned(),
            attribution: self.descriptor.attribution.clone(),
            download_bytes: self.descriptor.size,
        }
    }

    /// Whether this build understands the shape this entry is written in.
    /// [`Catalog::choose`] uses it to skip a model from the future rather than
    /// refusing the whole catalog.
    pub fn understood(&self) -> bool {
        self.descriptor.schema_version == DESCRIPTOR_SCHEMA_VERSION
    }
}

/// The list of models a build knows how to offer.
#[derive(Clone, PartialEq, Eq, Debug, Serialize, Deserialize)]
pub struct Catalog {
    pub schema_version: u32,
    pub entries: Vec<CatalogEntry>,
}

impl Catalog {
    /// Parse and structurally validate a catalog document.
    ///
    /// Signatures are **not** checked here. Verification is per entry, at the
    /// moment one is about to be installed, because a catalog whose fiftieth
    /// entry is signed by a retired key should still install the first
    /// forty-nine.
    pub fn parse(bytes: &[u8]) -> Result<Catalog, ModelError> {
        let catalog: Catalog = serde_json::from_slice(bytes)
            .map_err(|e| ModelErrorCode::CatalogInvalid.with(format!("catalog is invalid: {e}")))?;
        catalog.validate()?;
        Ok(catalog)
    }

    /// What is wrong for everybody, as opposed to what is wrong for this build.
    pub fn validate(&self) -> Result<(), ModelError> {
        if self.schema_version != CATALOG_SCHEMA_VERSION {
            return Err(ModelErrorCode::CatalogInvalid.with(format!(
                "catalog schema {} is not {CATALOG_SCHEMA_VERSION}",
                self.schema_version
            )));
        }
        for (at, entry) in self.entries.iter().enumerate() {
            for url in entry.sources() {
                require_https(url)?;
            }
            if self.entries[..at]
                .iter()
                .any(|other| other.id() == entry.id() && other.version() == entry.version())
            {
                return Err(ModelErrorCode::CatalogInvalid.with(format!(
                    "{}/{} is listed twice",
                    entry.id(),
                    entry.version()
                )));
            }
        }
        Ok(())
    }

    /// The entry naming exactly this model build, if the catalog has one.
    pub fn find(&self, id: &ModelId, version: &ModelVersion) -> Option<&CatalogEntry> {
        self.entries
            .iter()
            .find(|e| e.id() == id && e.version() == version)
    }

    /// **Choose a model for a language and a quality tier.**
    ///
    /// This is the resolution `koru_voice::SessionRequest` deliberately does
    /// not do — its doc says so: *"choosing one from a language and a quality
    /// tier is the model store's job, and a session that had to choose would
    /// need a catalog, which would put a downloader under a state machine that
    /// is meant to run on a phone's audio thread."* This is that job.
    ///
    /// Three rules, and each is a refusal to be clever:
    ///
    /// - **The tier is exact.** A `HighAccuracy` request that quietly settles
    ///   for `Compact` gives a person a worse result than they chose and no way
    ///   to find out. A caller that wants a fallback asks twice.
    /// - **`Requested::Auto` matches any model**, because whether a *language*
    ///   can be detected is `EngineInfo::detects_language`'s answer and not a
    ///   property of the weights.
    /// - **Catalog order is the publisher's preference order**, so the first
    ///   match wins. Deterministic, and it does not require inventing a
    ///   comparison over `ModelVersion`, which is an opaque label rather than
    ///   semver.
    pub fn choose(
        &self,
        requested: Requested,
        tier: ModelTier,
        engine: Option<&EngineId>,
    ) -> Option<&CatalogEntry> {
        self.entries.iter().find(|entry| {
            entry.understood()
                && entry.tier() == tier
                && entry.descriptor.covers(requested)
                && engine.is_none_or(|wanted| entry.engine() == wanted)
        })
    }
}

/// Where the catalog is published.
///
/// A constant rather than a configuration file: a build that could be pointed
/// at another catalog by anything short of an environment variable set on
/// purpose is a build somebody can be pointed away from. The signature is what
/// makes the *contents* trustworthy, and this is what makes the *default*
/// predictable.
pub const DEFAULT_CATALOG_URL: &str = "https://updates.koru.do/voice/catalog-v1.json";

/// A catalog document is a list of models, not a payload. A megabyte is far
/// more than a few dozen entries need and small enough that a server answering
/// something else cannot make this process reserve memory for it — the same
/// reasoning `koru_ipc::frame::MAX_BODY` is written up under.
const MAX_CATALOG_BYTES: u64 = 1024 * 1024;

/// **Where a catalog comes from**, resolved once rather than asked at each use.
///
/// Two forms and no third. A path is what a test, an air-gapped install and a
/// `--catalog` on a command line all name; a URL is the product path. Both
/// arrive through `KORU_VOICE_CATALOG`, which is a *path if it names one that
/// exists* and a URL otherwise — the ambiguity is resolved by the filesystem
/// rather than by a prefix somebody has to remember, and a value that is
/// neither is refused by name rather than fetched hopefully.
#[derive(Clone, PartialEq, Eq, Debug)]
pub enum CatalogSource {
    File(std::path::PathBuf),
    Url(Url),
}

impl CatalogSource {
    /// `KORU_VOICE_CATALOG`, then [`DEFAULT_CATALOG_URL`].
    ///
    /// **Reading the world**, which is why it is a resolved value: a caller
    /// that asked twice could be answered twice, and a download that verified
    /// against one catalog and installed from another would be a hole nobody
    /// meant to open.
    pub fn resolve() -> Result<CatalogSource, ModelError> {
        match koru_env::value(koru_env::name::KORU_VOICE_CATALOG) {
            Some(value) => CatalogSource::parse(&value),
            None => CatalogSource::parse(DEFAULT_CATALOG_URL),
        }
    }

    /// The rule with the value passed in, so it is testable without `set_var`
    /// — a process-global write that races every other test in the binary.
    pub fn parse(value: &str) -> Result<CatalogSource, ModelError> {
        let path = std::path::Path::new(value);
        if path.is_file() {
            return Ok(CatalogSource::File(path.to_path_buf()));
        }
        let url = Url::parse(value).map_err(|error| {
            ModelErrorCode::CatalogInvalid.with(format!(
                "{value:?} is neither a file that exists nor a URL: {error}"
            ))
        })?;
        require_https(&url)?;
        Ok(CatalogSource::Url(url))
    }

    /// **Fetch and parse it.** Blocking, and therefore not from a frame loop —
    /// see [`crate::ModelStore`]'s note about which threads may call the world.
    ///
    /// Local-first is not weakened by this existing: nothing calls it while a
    /// model is installed. A catalog is read when somebody is about to be
    /// *offered* a download, which is a moment they asked for.
    pub fn fetch(&self) -> Result<Catalog, ModelError> {
        match self {
            CatalogSource::File(path) => {
                let bytes = std::fs::read(path).map_err(|error| {
                    ModelErrorCode::CatalogInvalid
                        .with(format!("{} could not be read: {error}", path.display()))
                })?;
                Catalog::parse(&bytes)
            }
            CatalogSource::Url(url) => {
                let client = reqwest::blocking::Client::builder()
                    .connect_timeout(std::time::Duration::from_secs(5))
                    .timeout(std::time::Duration::from_secs(30))
                    // The same redirect policy the archive download uses, and
                    // copied rather than loosened: five hops, never off HTTPS.
                    .redirect(reqwest::redirect::Policy::custom(|attempt| {
                        let allowed = attempt.url().scheme() == "https"
                            || (cfg!(debug_assertions)
                                && matches!(
                                    attempt.url().host_str(),
                                    Some("localhost" | "127.0.0.1")
                                ));
                        if attempt.previous().len() >= 5 || !allowed {
                            attempt.stop()
                        } else {
                            attempt.follow()
                        }
                    }))
                    .build()
                    .map_err(|error| {
                        ModelErrorCode::Offline
                            .with(format!("could not build an HTTP client: {error}"))
                    })?;
                let response = client.get(url.clone()).send().map_err(|error| {
                    ModelErrorCode::Offline.with(format!("{url} could not be reached: {error}"))
                })?;
                if !response.status().is_success() {
                    // **Answered, and not with a catalog.** A separate code
                    // from a malformed body on purpose: a 404 means nothing is
                    // published at this address, which is a different sentence
                    // and a different thing to check.
                    return Err(ModelErrorCode::CatalogUnavailable
                        .with(format!("{url} answered {}", response.status())));
                }
                // Checked before the body is read as well as after, because a
                // declared length is what stops the read rather than what
                // proves it.
                if response
                    .content_length()
                    .is_some_and(|n| n > MAX_CATALOG_BYTES)
                {
                    return Err(ModelErrorCode::CatalogInvalid
                        .with(format!("{url} declares more than a catalog can be")));
                }
                let bytes = response.bytes().map_err(|error| {
                    ModelErrorCode::Offline.with(format!("{url} was cut off: {error}"))
                })?;
                if bytes.len() as u64 > MAX_CATALOG_BYTES {
                    return Err(ModelErrorCode::CatalogInvalid
                        .with(format!("{url} answered more than a catalog can be")));
                }
                Catalog::parse(&bytes)
            }
        }
    }
}

/// HTTPS, or loopback in a debug build.
///
/// The same rule `koru_update::controller::download_verified` already applies,
/// copied because it is the rule and not because it is convenient: a release
/// build must not be talked into plain HTTP by a catalog, and a test suite must
/// be able to stand up a fixture server without a certificate. Copying the
/// *exception* as well is the point — a stricter rule here would make Plan
/// 1.2's local fixture server unbuildable and would be relaxed under pressure
/// into something looser than this.
fn require_https(url: &Url) -> Result<(), ModelError> {
    let loopback =
        cfg!(debug_assertions) && matches!(url.host_str(), Some("localhost" | "127.0.0.1"));
    if url.scheme() == "https" || loopback {
        return Ok(());
    }
    Err(ModelErrorCode::CatalogInvalid.with(format!("{url} must use HTTPS")))
}

#[cfg(test)]
mod tests;
