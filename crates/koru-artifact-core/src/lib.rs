// SPDX-License-Identifier: MPL-2.0

//! What an artifact is.
//!
//! A delegated run ends in a result a person can read, trust and use again
//! (ROADMAP §1). This crate is the vocabulary of that result: a **family** is
//! the thing a person names — "the quarterly report" — and a **version** is
//! one immutable set of bytes registered under it. The bytes themselves live
//! in the workspace's content-addressed blob store and are named here by a
//! [`BlobHash`]; the version's *identity* is an [`ArtifactVersionId`], minted
//! separately, so two registrations of the same bytes are two versions and a
//! reference to a version means one thing forever (D1, D6).
//!
//! Three rules shape everything here:
//!
//! - **A version is immutable.** There is no operation on the port that
//!   changes one; the database refuses an `UPDATE` with a trigger, and the
//!   service refuses it with the same stable code
//!   ([`ArtifactError::VersionImmutable`]). A family is *retired*, never
//!   erased (PRINCIPLES, *retire state*).
//! - **A registration is idempotent by its own id.** A retried
//!   [`RegistrationId`] finds the version it made rather than making a second
//!   one — the row §7.1 of the roadmap puts under `artifacts.db`.
//! - **The permission vocabulary exists before it is reachable.** [`Access`]
//!   and [`Principal`] are modeled now so M7's replication has words to use;
//!   in M1 only the local owner is reachable (D11), and
//!   [`Principal::reachable`] says so.
//! - **A run's authority is a record, resolved at every call.** What a
//!   delegated agent may do is a [`RunAuthorityRecord`] keyed by the hash of
//!   the token it presents — never by anything the call says about itself —
//!   and [`authorize_family`] is the one grant rule over it (§7.3, D28).
//!
//! Privacy is structural, as in `koru-delegate-core`: a title and a filename
//! are content, and their `Debug` redacts, so no derived `Debug` above them
//! can leak one. This crate holds no bytes and no HTTP.

mod access;
mod authority;
mod error;
mod family;
mod ids;
mod ports;
mod provenance;
mod text;
mod version;

pub use access::{Access, Permission, Principal, PrincipalParseError, authorize};
pub use authority::{
    ARTIFACTS_DIR, AuthorityRefusal, AuthorityState, NewAuthority, RunAuthorityRecord,
    RunWorkspace, TokenHash, TokenLookup, artifacts_folder, authorize_family,
};
pub use error::ArtifactError;
pub use family::{ArtifactFamily, NewFamily};
pub use ids::{ArtifactFamilyId, RegistrationId};
pub use ports::{ArtifactRepository, RunAuthorityRepository};
pub use provenance::{LinkRole, Provenance, RunLink};
pub use text::{
    Filename, MAX_FILENAME_CHARS, MAX_MEDIA_TYPE_CHARS, MAX_RELATIVE_PATH_CHARS, MediaType,
    MediaTypeError, RelativePath, RelativePathError,
};
pub use version::{ArtifactVersion, NewVersion, Ordinal, ProducedBy, Registered};

// The vocabulary this crate speaks but does not own, re-exported so a
// dependant names one crate for an artifact and its parts.
pub use koru_delegate_core::{
    ArtifactTitle, ArtifactVersionId, AttemptGeneration, BlobHash, DeviceId, IdParseError,
    MAX_ARTIFACT_TITLE_CHARS, ProfileId, RunId, TextError, WorkspaceId,
};

/// `Result` over the artifact error vocabulary.
pub type Result<T> = std::result::Result<T, ArtifactError>;

#[cfg(test)]
mod tests;
