// SPDX-License-Identifier: MPL-2.0

//! **What a person agreed to before Koru downloads a speech model or opens a
//! microphone.** One machine-local file, versioned, written atomically, per OS
//! user and device.
//!
//! ## Why this is a crate and not a field in `preferences.conf`
//!
//! `preferences.conf` follows the person — theme, font, zoom — and roams.
//! **Consent given on a desktop is not consent given on a laptop**: a different
//! machine has a different microphone, a different network, and quite possibly
//! a different person sitting at it. So the record lives beside the other
//! machine-local state (`windows.conf`, `search.conf`, `renderer.conf`) and it
//! is keyed by nothing at all: one OS user, one device, one answer. It is
//! deliberately **not** per profile and not per workspace — a workspace is
//! content, and agreeing to download 650 MB is not a fact about content.
//!
//! ## Consent is not installed-state
//!
//! These are two facts and this crate holds exactly one of them. *What somebody
//! agreed to* — the download size and the model's licence, at a recorded
//! version — is a promise a person made, and it survives the bytes. *Whether
//! verified bytes are on the disk* is a question about a directory, and it is
//! answered by asking the directory
//! (`koru_voice_models::anything_installed`), never by a boolean here that
//! could drift from the disk it describes. The two consequences are the ones
//! the plan asks for and they fall out of the split rather than being
//! maintained: **deleting the model does not revoke consent**, and **revoking
//! consent does not delete the model.**
//!
//! ## A torn write is not consent
//!
//! Three mechanisms, and each of them is load-bearing.
//!
//! - **The write is atomic.** [`grant`] writes a sibling temporary file,
//!   `sync_all`s it, and renames it over the record. A reader therefore sees
//!   the whole of one version or the whole of the previous one, never half of
//!   each — `std::fs::rename` replaces an existing file on Windows as well as
//!   on Unix, which is the one platform detail this depends on.
//! - **Reading is total, and every failure means *not consented*.**
//!   [`read`] cannot fail. A missing file, an unreadable one, JSON that does
//!   not parse, a record from a schema this build does not know: all of them
//!   answer [`Consent::Absent`] with a [`Reason`], so a caller has nothing to
//!   get wrong. The alternative — a `Result` — puts the decision at the call
//!   site, and the call site that decides wrongly downloads something nobody
//!   asked for.
//! - **The file names itself.** [`FORMAT`] is the first field and is checked,
//!   so a file of zeros, a truncated write and some *other* JSON that happens
//!   to be here are all refused rather than partially believed.
//!
//! ## Versioned, with the migration named
//!
//! [`SCHEMA_VERSION`] is 1 and there is no migration yet, which is a statement
//! rather than an omission: a record from a **newer** build is
//! [`Reason::UnknownSchema`] — *not consented* — and it is left on the disk
//! rather than overwritten, so downgrading and upgrading again does not lose
//! what somebody agreed to. Only a person granting again writes over it, which
//! is safe because they were just asked. A record from an **older** build, when
//! there is one, is migrated forward here and nowhere else.
//!
//! ## The token, and what it is honestly worth
//!
//! [`Granted`] is what every path that downloads a model or opens a microphone
//! asks for by value, so *"no path reaches a download or a microphone without
//! consent"* is a signature rather than a rule somebody has to remember. It is
//! not a capability in the cryptographic sense: [`Granted::recorded`] builds
//! one in memory, because the consent step holds a grant between the moment
//! somebody presses the button and the moment the record reaches the disk, and
//! a type that could only come from a file would make that moment
//! unrepresentable. What the token buys is that a caller cannot *forget* — it
//! has to say the word `consent` to compile — and that is the failure this
//! guards against.

use serde::{Deserialize, Serialize};
use std::path::{Path, PathBuf};

/// The record's file name. **The directory is the host's**, for the reason
/// `search.conf`'s is
/// ([ADR-111](../../../docs/decisions/ADR-101-125.md)): an agent under `--root`
/// writes under the root, and a test names a temporary directory, so a crate
/// that resolved its own path would make both of those special cases.
pub const CONSENT_FILE: &str = "voice-consent.conf";

/// What the file says it is, in its first field. Checked on every read, so a
/// file of zeros or somebody else's JSON is refused rather than half believed.
pub const FORMAT: &str = "koru-voice-consent";

/// The version of the record's shape. See the module note on migration.
pub const SCHEMA_VERSION: u32 = 2;

/// **What the person was shown**, and therefore what they agreed to.
///
/// Strings rather than the model store's newtypes, deliberately. A `ModelId` is
/// a validated path component in a cache this crate has never heard of; what is
/// recorded here is the text that was on the screen, which has to stay readable
/// long after the model it named was pruned. The store builds one of these from
/// a catalog entry (`CatalogEntry::terms`) so the two cannot disagree about
/// what an offer said.
#[derive(Clone, PartialEq, Eq, Debug, Serialize, Deserialize)]
pub struct Terms {
    /// The model the offer named — an id, as the catalog spells it.
    pub model: String,
    /// Which build of it.
    pub version: String,
    /// The SPDX identifier the weights are published under. **The half of the
    /// offer that binds**: see [`Granted::covers`].
    pub licence: String,
    /// Who the licence requires be credited.
    pub attribution: String,
    /// The download, in bytes, exactly as it was shown.
    pub download_bytes: u64,
}

impl Terms {
    /// The download in whole megabytes, the way a sentence says it.
    ///
    /// Here rather than at the surface because three surfaces print it — the
    /// tray row, the consent step and the settings section — and three
    /// roundings would eventually disagree by a megabyte in front of somebody
    /// deciding whether to spend it.
    pub fn megabytes(&self) -> u64 {
        megabytes(self.download_bytes)
    }
}

/// The same rounding, for a size that never travelled as [`Terms`].
///
/// The settings section prints what is *installed*, which is a sum over signed
/// descriptors rather than a recorded offer — so it has no `Terms` to ask, and
/// the note above ("three roundings would eventually disagree by a megabyte")
/// applies to it exactly as much. One expression, two callers.
pub fn megabytes(bytes: u64) -> u64 {
    bytes.div_ceil(1024 * 1024)
}

/// **Proof that somebody agreed.** Asked for by value by every path that
/// downloads a model or opens a microphone — see the module note for what that
/// is and is not worth.
#[derive(Clone, PartialEq, Eq, Debug)]
pub struct Granted {
    grants: Vec<Grant>,
}

impl Granted {
    /// A grant that has not reached the disk yet — what the consent step holds
    /// between the button and the write.
    pub fn recorded(terms: Terms, granted_at: u64) -> Granted {
        Granted::recorded_all(vec![terms], granted_at)
    }

    /// Several offers accepted in one consent step.
    pub fn recorded_all(terms: Vec<Terms>, granted_at: u64) -> Granted {
        Granted {
            grants: terms
                .into_iter()
                .map(|terms| Grant { terms, granted_at })
                .collect(),
        }
    }

    /// What was agreed to.
    pub fn terms(&self) -> &Terms {
        &self.grants[0].terms
    }

    pub fn all_terms(&self) -> impl Iterator<Item = &Terms> {
        self.grants.iter().map(|grant| &grant.terms)
    }

    /// When, in seconds since the epoch. Informational: nothing expires, and a
    /// clock that went backwards must not revoke anything.
    pub fn granted_at(&self) -> u64 {
        self.grants
            .iter()
            .map(|grant| grant.granted_at)
            .max()
            .unwrap_or(0)
    }

    /// **Does this grant cover that offer?**
    ///
    /// Two rules, and both are refusals to be clever.
    ///
    /// - **The licence must be the same.** Agreeing to Apache-2.0 weights is
    ///   not agreeing to weights under a licence with a field-of-use
    ///   restriction, and a build that treated one as the other would have
    ///   somebody's name on a compliance question they never answered.
    /// - **The download may not be larger.** The size is on the screen because
    ///   people pay for bytes; a model that grew by 400 MB between versions is
    ///   a new question. A *smaller* one is not, which is why this is `<=`
    ///   rather than `==`.
    ///
    /// Everything else — a new version, a new model id under the same licence
    /// and no larger — is covered, because re-asking the same question is how
    /// a consent step teaches people to press the button without reading it.
    pub fn covers(&self, offer: &Terms) -> bool {
        self.grants.iter().any(|grant| {
            offer.licence == grant.terms.licence
                && offer.download_bytes <= grant.terms.download_bytes
        })
    }
}

/// Why there is no consent to be had.
///
/// Every one of these means *not consented*; they are separate so a log line
/// can say which, and because two of them mean something is wrong with the file
/// rather than with the person's answer.
#[derive(Copy, Clone, PartialEq, Eq, Debug)]
pub enum Reason {
    /// Nobody has been asked on this machine.
    NeverAsked,
    /// Somebody was asked, and said no — or took it back afterwards. A
    /// *recorded* answer, which is why it is not [`Reason::NeverAsked`].
    Revoked,
    /// The file is there and could not be read: a permission, a directory where
    /// a file should be, a disk that is failing.
    Unreadable,
    /// The bytes are not a consent record — truncated, zeroed, or some other
    /// file that arrived under this name. **This is what a torn write looks
    /// like**, and it is why it is not consent.
    Malformed,
    /// A record from a build newer than this one. Never read as consent, and
    /// never overwritten except by somebody granting again.
    UnknownSchema,
}

impl Reason {
    /// The stable token a log line carries.
    pub fn as_str(self) -> &'static str {
        match self {
            Reason::NeverAsked => "never_asked",
            Reason::Revoked => "revoked",
            Reason::Unreadable => "unreadable",
            Reason::Malformed => "malformed",
            Reason::UnknownSchema => "unknown_schema",
        }
    }
}

impl std::fmt::Display for Reason {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(self.as_str())
    }
}

/// What the record says, as a value a caller cannot ignore by accident.
#[derive(Clone, PartialEq, Eq, Debug)]
pub enum Consent {
    Granted(Granted),
    Absent(Reason),
}

impl Consent {
    /// The token, when there is one. The whole of what a gate needs.
    pub fn granted(&self) -> Option<&Granted> {
        match self {
            Consent::Granted(granted) => Some(granted),
            Consent::Absent(_) => None,
        }
    }

    /// Why not, when there is not. For a log line, never for a person.
    pub fn reason(&self) -> Option<Reason> {
        match self {
            Consent::Granted(_) => None,
            Consent::Absent(reason) => Some(*reason),
        }
    }

    pub fn is_granted(&self) -> bool {
        matches!(self, Consent::Granted(_))
    }
}

/// The record exactly as it is on the disk.
///
/// `granted: None` is a **recorded refusal**, which is a different fact from
/// the file not being there: it is what [`revoke`] writes, and it is why
/// revoking is not implemented as deleting the file.
#[derive(Clone, PartialEq, Eq, Debug, Serialize, Deserialize)]
struct Record {
    format: String,
    schema_version: u32,
    grants: Vec<Grant>,
}

#[derive(Clone, PartialEq, Eq, Debug, Serialize, Deserialize)]
struct RecordV1 {
    format: String,
    schema_version: u32,
    granted: Option<Grant>,
}

#[derive(Clone, PartialEq, Eq, Debug, Serialize, Deserialize)]
struct Grant {
    terms: Terms,
    granted_at: u64,
}

/// **Read the record. This cannot fail.**
///
/// Every way of not being able to read consent is *not consented* — see the
/// module note on why this is total rather than a `Result`.
pub fn read(path: &Path) -> Consent {
    let bytes = match std::fs::read(path) {
        Ok(bytes) => bytes,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
            return Consent::Absent(Reason::NeverAsked);
        }
        Err(_) => return Consent::Absent(Reason::Unreadable),
    };
    let value: serde_json::Value = match serde_json::from_slice(&bytes) {
        Ok(record) => record,
        // A truncated write, a file of zeros, somebody else's JSON. All of
        // them arrive here and none of them is an answer somebody gave.
        Err(_) => return Consent::Absent(Reason::Malformed),
    };
    if value.get("format").and_then(|value| value.as_str()) != Some(FORMAT) {
        return Consent::Absent(Reason::Malformed);
    }
    match value.get("schema_version").and_then(|value| value.as_u64()) {
        Some(1) => match serde_json::from_value::<RecordV1>(value) {
            Ok(record) => match record.granted {
                Some(grant) => Consent::Granted(Granted {
                    grants: vec![grant],
                }),
                None => Consent::Absent(Reason::Revoked),
            },
            Err(_) => Consent::Absent(Reason::Malformed),
        },
        Some(version) if version == u64::from(SCHEMA_VERSION) => {
            match serde_json::from_value::<Record>(value) {
                Ok(record) if record.grants.is_empty() => Consent::Absent(Reason::Revoked),
                Ok(record) => Consent::Granted(Granted {
                    grants: record.grants,
                }),
                Err(_) => Consent::Absent(Reason::Malformed),
            }
        }
        _ => Consent::Absent(Reason::UnknownSchema),
    }
}

/// **Record that somebody agreed**, and answer the token.
///
/// Atomic: a sibling temporary file, `sync_all`, then a rename over the
/// record. The parent directory is created if it is not there, because the
/// machine-local directory is made by whichever Koru process needs it first.
pub fn grant(path: &Path, terms: Terms, granted_at: u64) -> std::io::Result<Granted> {
    grant_all(path, vec![terms], granted_at)
}

/// Record several offers accepted together.
pub fn grant_all(path: &Path, terms: Vec<Terms>, granted_at: u64) -> std::io::Result<Granted> {
    let granted = Granted::recorded_all(terms, granted_at);
    write_record(
        path,
        &Record {
            format: FORMAT.to_owned(),
            schema_version: SCHEMA_VERSION,
            grants: granted.grants.clone(),
        },
    )?;
    Ok(granted)
}

/// **Take it back.** Writes a recorded refusal rather than deleting the file,
/// so *asked and said no* stays distinguishable from *never asked*.
///
/// It deletes no model. The bytes on the disk are a separate fact and this
/// crate does not know where they are.
pub fn revoke(path: &Path) -> std::io::Result<()> {
    write_record(
        path,
        &Record {
            format: FORMAT.to_owned(),
            schema_version: SCHEMA_VERSION,
            grants: Vec::new(),
        },
    )
}

/// Seconds since the epoch, for a caller that wants the ordinary answer.
///
/// A clock before 1970 answers 0 rather than panicking: the stamp is
/// informational, and a machine with a wrong clock must still be able to
/// consent.
pub fn now_unix() -> u64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|since| since.as_secs())
        .unwrap_or(0)
}

/// The record's path inside a directory the host named.
pub fn path_in(dir: &Path) -> PathBuf {
    dir.join(CONSENT_FILE)
}

/// Write, flush, rename. The one place the atomicity lives.
fn write_record(path: &Path, record: &Record) -> std::io::Result<()> {
    use std::io::Write as _;

    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent)?;
    }
    // Beside the record rather than in a temp directory: a rename across
    // volumes is a copy, and a copy is not atomic.
    let staging = path.with_extension("conf.new");
    let bytes = serde_json::to_vec_pretty(record)
        .map_err(|error| std::io::Error::new(std::io::ErrorKind::InvalidData, error))?;
    {
        let mut file = std::fs::File::create(&staging)?;
        file.write_all(&bytes)?;
        file.write_all(b"\n")?;
        // **Before the rename, not after.** A rename that beat its own bytes to
        // the disk is the torn write this whole module is about.
        file.sync_all()?;
    }
    match std::fs::rename(&staging, path) {
        Ok(()) => Ok(()),
        Err(error) => {
            let _ = std::fs::remove_file(&staging);
            Err(error)
        }
    }
}

#[cfg(test)]
mod tests;
