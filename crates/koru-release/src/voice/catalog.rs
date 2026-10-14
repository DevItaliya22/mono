// SPDX-License-Identifier: MPL-2.0

//! **What gets written to the origin: the keys, and the JSON at each of them.**
//!
//! Every document here is built by constructing the model store's **own**
//! types and serialising those — never by assembling JSON against a shape this
//! crate cannot see. That is the `koru-store-wire` lesson applied a second
//! time: the client that sent `serde_json::json!` literals against types it
//! could not name drifted from the server twice, and the fix was to make a
//! renamed field break both builds. Here it breaks this one, at the moment
//! somebody edits `koru-voice-models`, rather than producing a catalog the
//! desktop silently declines.
//!
//! ## The three keys
//!
//! | Key | What | Cached |
//! | --- | --- | --- |
//! | `models/<id>/<version>/<digest>.zip` | the archive | immutable, one year |
//! | `models/<id>/<version>.json` | one `CatalogEntry` | 30 s at the edge |
//! | `voice/catalog-v1.json` | the `Catalog` | 30 s at the edge |
//!
//! **The archive is content-addressed and the descriptor is not**, and that
//! difference is the whole cache story. The digest is in the archive's key, so
//! those bytes can never change under that name and a year is safe. The
//! descriptor's key names only the version, and the two fields *outside* the
//! signature envelope — `url` and `mirrors` — exist precisely so a download
//! host can be added or withdrawn without re-signing anything. A descriptor
//! behind a one-year browser TTL could not be corrected at all, and a browser
//! TTL is not reachable by a purge. `index.rs` learned that the expensive way.
//!
//! **Nothing at the edge overrides these headers.** `cloud-setup`'s cache rules
//! match `/releases/` and `/channels/` and nothing else, so `/models/` and
//! `/voice/` are served with whatever `Cache-Control` the upload carried. That
//! is why the values are here and not in a ruleset — and it is also the thing
//! to check first if a published catalog seems stale.
//!
//! ## One host, decided rather than inherited
//!
//! Both the store's [`DEFAULT_CATALOG_URL`] and this tool name
//! `updates.koru.do`, so a published catalog is the one a stock build reads.
//! The alternative — `models.koru.do` as a second custom domain on the same
//! bucket — was considered and declined: it is a Cloudflare configuration to
//! keep working and a second name to renew, in exchange for separating model
//! traffic from update traffic in analytics. One bucket, one host, one set of
//! cache rules.
//!
//! [`catalog_host_warning`] stays, and answers `None` while they agree. It is a
//! comparison rather than a deletion so that moving either host later is loud on
//! the next run instead of silently publishing a catalog nothing reads.

use super::table::ModelArtifact;
use crate::cloud::HOST;
use koru_voice::{EngineId, ModelId};
use koru_voice_models::{
    CATALOG_SCHEMA_VERSION, Catalog, CatalogEntry, DEFAULT_CATALOG_URL, DESCRIPTOR_SCHEMA_VERSION,
    LicenceRef, ModelDescriptor, ModelSignature, ModelVersion, Sha256Digest,
};

/// Where the catalog document goes. The path half of
/// [`DEFAULT_CATALOG_URL`], so only the host has to change later.
pub const CATALOG_KEY: &str = "voice/catalog-v1.json";

/// Immutable, because the key holds the digest. The same string `dist-cloud`
/// puts on a release package.
pub const ARCHIVE_CACHE: &str = "public, max-age=31536000, immutable";

/// Short, because a descriptor's unsigned fields are meant to be changeable.
/// The same value the channel manifests carry.
pub const DOCUMENT_CACHE: &str = "no-cache, max-age=30";

/// **The signature field, when there is no signature.**
///
/// `MODEL_SIGNING_KEYS` is empty and staying empty for the MVP — minting the
/// key and holding its private half is an operations decision the owner has
/// deferred until before public release — so nothing here signs anything, and
/// this tool deliberately does not mint a key of its own.
///
/// `ModelSignature::parse` refuses an empty string, so the field cannot simply
/// be omitted, and it is not `Option` in `CatalogEntry`. What goes there is a
/// sentence rather than a plausible-looking blob: under `TrustPolicy::SIGNED`
/// it fails as it must, under `TrustPolicy::digest_only` it is skipped and
/// stored verbatim in the installed metadata, and in both cases anybody who
/// reads it learns why it is not a signature.
///
/// **When the key is minted this constant is deleted, not edited.**
pub const UNSIGNED: &str = "UNSIGNED: no Koru model-signing key has been minted yet, so this \
                            entry carries no signature and only its digest was checked. See \
                            docs/features/voice/archive/QUESTIONS.md Q-V14 and Q-V28.";

/// `models/<id>/<version>/<digest>.zip`.
pub fn archive_key(artifact: &ModelArtifact, digest: &str) -> String {
    format!("models/{}/{}/{digest}.zip", artifact.id, artifact.version)
}

/// `models/<id>/<version>.json` — the layout `ROADMAP.rev1` named, holding one
/// `CatalogEntry` so that fetching one model needs no whole catalog.
pub fn descriptor_key(artifact: &ModelArtifact) -> String {
    format!("models/{}/{}.json", artifact.id, artifact.version)
}

/// The public URL an entry points a downloader at.
pub fn archive_url(artifact: &ModelArtifact, digest: &str) -> String {
    format!("https://{HOST}/{}", archive_key(artifact, digest))
}

/// The catalog's public URL on the host this tool actually writes to.
pub fn catalog_url() -> String {
    format!("https://{HOST}/{CATALOG_KEY}")
}

/// `None` when the store's default already names this host.
///
/// A function rather than a constant so that the day `models.koru.do` is added
/// to the bucket, the warning disappears by itself instead of being deleted by
/// somebody who has to notice it is stale.
pub fn catalog_host_warning() -> Option<String> {
    if DEFAULT_CATALOG_URL == catalog_url() {
        return None;
    }
    Some(format!(
        "the desktop reads {DEFAULT_CATALOG_URL} by default, and this publishes {}.\n    \
         Until models.koru.do is a second custom domain on the same bucket, a build only\n    \
         sees this catalog through KORU_VOICE_CATALOG. See the archived voice question log, Q-V78.",
        catalog_url()
    ))
}

/// **One catalog entry, built through the store's own types.**
///
/// Every newtype here checks its own rule at construction, so a bad id, an
/// uppercase digest, a licence with a tab in it or a version that is not a
/// legal path component fails on this machine rather than in somebody's
/// installer. `ModelDescriptor::validate` then checks what no single field can
/// know, and it is called here rather than trusted to the reader.
pub fn entry(artifact: &ModelArtifact, size: u64, digest: &str) -> Result<CatalogEntry, String> {
    let descriptor = ModelDescriptor {
        schema_version: DESCRIPTOR_SCHEMA_VERSION,
        id: ModelId::parse(artifact.id).map_err(|error| error.to_string())?,
        version: ModelVersion::parse(artifact.version).map_err(|error| error.to_string())?,
        engine: EngineId::parse(artifact.engine).map_err(|error| error.to_string())?,
        tier: artifact.tier,
        languages: artifact.languages()?,
        size,
        digest: Sha256Digest::parse(digest).map_err(|error| error.to_string())?,
        working_set_bytes: artifact.working_set_bytes,
        licence: LicenceRef::parse(artifact.licence).map_err(|error| error.to_string())?,
        attribution: artifact.attribution.to_string(),
    };
    descriptor.validate().map_err(|error| error.to_string())?;
    // Rendering the canonical bytes is free and is the one check that the
    // descriptor could be signed the day a key exists — a field carrying a tab
    // or an unescapable control character fails here rather than at signing.
    descriptor
        .canonical_bytes()
        .map_err(|error| error.to_string())?;
    Ok(CatalogEntry {
        signature: ModelSignature::parse(UNSIGNED).map_err(|error| error.to_string())?,
        url: archive_url(artifact, digest)
            .parse()
            .map_err(|error| format!("{}: {error}", artifact.id))?,
        mirrors: Vec::new(),
        descriptor,
    })
}

/// The catalog document, validated before it is rendered.
///
/// `Catalog::validate` is what refuses a duplicated `id`/`version` pair and a
/// URL that is not HTTPS — the same two checks every desktop will run on the
/// way in, run here so a broken catalog never reaches the origin.
pub fn catalog(entries: Vec<CatalogEntry>) -> Result<Catalog, String> {
    let catalog = Catalog {
        schema_version: CATALOG_SCHEMA_VERSION,
        entries,
    };
    catalog.validate().map_err(|error| error.to_string())?;
    Ok(catalog)
}

/// Pretty JSON with a trailing newline.
///
/// Pretty rather than compact because this document is read by people during a
/// publish far more often than it is fetched by a machine, and 30 KB against
/// 20 KB over a 30-second cache is not a trade worth making.
pub fn render<T: serde::Serialize>(value: &T) -> Result<Vec<u8>, String> {
    let mut bytes = serde_json::to_vec_pretty(value).map_err(|error| error.to_string())?;
    bytes.push(b'\n');
    Ok(bytes)
}
