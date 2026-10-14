// SPDX-License-Identifier: MPL-2.0

//! The checked-in contract vectors, typed.
//!
//! `cases.json` and `sequences.json` are **read by the test suite, not only by
//! people**, and they are hand-written because they encode intent — a generator
//! would only ever reproduce the implementation's own assumptions. They are
//! compiled in with `include_str!` so a test never reads the working tree, and
//! the shapes below use `deny_unknown_fields` on purpose: adding a field to a
//! vector is adding a sentence to the contract, so it should fail to build
//! until somebody has said what the field means here.

use koru_device::cloud::CloudId;
use serde::Deserialize;

pub const CASES_JSON: &str =
    include_str!("../../../contracts/vectors/cloud-blob-object-v1/cases.json");
pub const SEQUENCES_JSON: &str =
    include_str!("../../../contracts/vectors/cloud-blob-object-v1/sequences.json");

/// Which family a case belongs to, which is also which adapter owns it.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum CaseKind {
    /// Capability admission — the object worker, over HTTP, except the three
    /// bindings only a database can decide.
    Capability,
    /// `Content-Range` admission — the object worker.
    Range,
    /// Allocation — the control API.
    Allocate,
    /// Verification — the object worker.
    Verify,
    /// Reads — the object worker.
    Get,
    /// The report seam — the control API.
    Report,
}

/// The claims a case's capability carries. Everything absent is filled from the
/// adapter's own well-formed baseline, so a case says only what it changes.
#[derive(Clone, Debug, Default, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct CaseClaims {
    pub v: Option<u32>,
    pub pur: Option<String>,
    pub rgn: Option<String>,
    pub aud: Option<String>,
    pub sub: Option<String>,
    pub bh: Option<String>,
    pub len: Option<u64>,
    pub eph: Option<i64>,
    pub inc: Option<String>,
    pub wsp: Option<String>,
}

/// One admission decision.
///
/// Fields beyond `claims` describe the *world* the request meets rather than
/// the request itself: `current_eph` and `current_inc` are what the database
/// now says, `route_sub` is the id in the URL when it differs from the
/// capability's, and `identical` says whether a replayed range carries the same
/// bytes.
#[derive(Clone, Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Case {
    pub id: String,
    pub kind: CaseKind,
    pub note: String,
    /// The expected stable code, or `ok`.
    pub code: String,

    // -- capability
    #[serde(default)]
    pub claims: Option<CaseClaims>,
    /// The deployment the request is made against, when it is not the US one.
    #[serde(default)]
    pub at: Option<String>,
    /// The byte operation the capability is used for, when the case is about
    /// using it for the wrong one.
    #[serde(default)]
    pub op: Option<String>,
    #[serde(default)]
    pub expired: Option<bool>,
    #[serde(default)]
    pub not_before: Option<bool>,
    #[serde(default)]
    pub tamper: Option<String>,
    #[serde(default)]
    pub route_sub: Option<String>,
    #[serde(default)]
    pub session_wsp: Option<String>,
    #[serde(default)]
    pub current_inc: Option<String>,
    #[serde(default)]
    pub current_eph: Option<i64>,
    /// An `Authorization` scheme other than `Capability`.
    #[serde(default)]
    pub scheme: Option<String>,

    // -- range
    #[serde(default)]
    pub committed: Option<u64>,
    /// `[start, end]`, inclusive, signed because a vector may express an empty
    /// or inverted range that no `u64` pair could hold.
    #[serde(default)]
    pub range: Option<[i64; 2]>,
    #[serde(default)]
    pub total: Option<u64>,
    #[serde(default)]
    pub identical: Option<bool>,
    /// A `Content-Range` header supplied verbatim because it does not parse.
    #[serde(default)]
    pub raw: Option<String>,
    #[serde(default)]
    pub body_len: Option<u64>,
    #[serde(default)]
    pub cap_len: Option<u64>,
    #[serde(default)]
    pub committed_after: Option<u64>,

    // -- allocate
    #[serde(default)]
    pub size: Option<u64>,
    #[serde(default)]
    pub existing: Option<String>,
    #[serde(default)]
    pub durable: Option<bool>,
    #[serde(default)]
    pub other_workspace: Option<bool>,
    #[serde(default)]
    pub same_session: Option<bool>,
    #[serde(default)]
    pub state: Option<String>,

    // -- verify
    #[serde(default)]
    pub matches: Option<bool>,
    #[serde(default)]
    pub publishes: Option<bool>,
    #[serde(default)]
    pub quarantines: Option<bool>,
    #[serde(default)]
    pub short: Option<bool>,

    // -- get
    #[serde(default)]
    pub exists: Option<bool>,
    #[serde(default)]
    pub mapping: Option<bool>,
    #[serde(default)]
    pub bytes: Option<bool>,

    // -- report
    #[serde(default)]
    pub repeat: Option<bool>,
    #[serde(default)]
    pub rows_after: Option<u32>,
    #[serde(default)]
    pub epoch_moved: Option<bool>,
    #[serde(default)]
    pub aborted: Option<bool>,
    #[serde(default)]
    pub credential: Option<bool>,
}

impl Case {
    /// The expected code, or `None` when the case expects admission.
    pub fn expected(&self) -> Option<crate::BlobCode> {
        if self.code == "ok" {
            return None;
        }
        Some(
            crate::BlobCode::parse(&self.code)
                .unwrap_or_else(|| panic!("case {} names code {}", self.id, self.code)),
        )
    }
}

/// The alternative ending a sequence carries when one run has to demonstrate
/// both halves of a rule — S12's session that keeps progressing.
#[derive(Clone, Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct SequenceVariant {
    pub steps: Vec<String>,
    #[serde(default)]
    pub reclaims: Option<Vec<String>>,
}

/// One run with a kill in it. The assertions that matter are not "it finished"
/// but the invariants beside it.
#[derive(Clone, Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Sequence {
    pub id: String,
    pub note: String,
    pub steps: Vec<String>,
    /// The step after which the blob is durable, or explicitly `null` for a run
    /// that must never reach durability.
    #[serde(default)]
    pub durable_after: Option<String>,
    /// **SQL never claims durability before the immutable object exists.**
    #[serde(default)]
    pub mapping_before_bytes: Option<bool>,
    /// The client-side twin: a successful metadata lookup can never point at a
    /// partial file.
    #[serde(default)]
    pub association_before_bytes: Option<bool>,
    #[serde(default)]
    pub head_returns: Option<String>,
    /// A resume re-sends nothing.
    #[serde(default)]
    pub resends: Option<u32>,
    #[serde(default)]
    pub reverifies: Option<bool>,
    #[serde(default)]
    pub bytes_flow_while_api_down: Option<bool>,
    #[serde(default)]
    pub reclaims: Option<Vec<String>>,
    /// Two devices racing on one picture converge: one session, one locator.
    #[serde(default)]
    pub sessions: Option<u32>,
    #[serde(default)]
    pub locators: Option<u32>,
    #[serde(default)]
    pub publishes: Option<bool>,
    #[serde(default)]
    pub renders: Option<bool>,
    #[serde(default)]
    pub variant: Option<SequenceVariant>,
}

pub fn cases() -> Vec<Case> {
    serde_json::from_str(CASES_JSON).expect("the checked-in cases.json is the contract's")
}

pub fn sequences() -> Vec<Sequence> {
    serde_json::from_str(SEQUENCES_JSON).expect("the checked-in sequences.json is the contract's")
}

/// Turn a vector's schematic label — `upload-1`, `ws-2`, `inc-1`, `loc-1` —
/// into a stable v4-shaped id.
///
/// The vectors name identities the way a sentence does; the system names them
/// the way a database column does. This is the one place the two meet, and it
/// is deterministic so a failing case names the same id on every machine and in
/// every rerun.
pub fn fixture_id(label: &str) -> CloudId {
    let mix = |seed: u64| -> u64 {
        let mut hash = seed;
        for byte in label.as_bytes() {
            hash ^= u64::from(*byte);
            hash = hash.wrapping_mul(0x0000_0100_0000_01b3);
        }
        hash
    };
    let mut bytes = [0u8; 16];
    bytes[..8].copy_from_slice(&mix(0xcbf2_9ce4_8422_2325).to_be_bytes());
    bytes[8..].copy_from_slice(&mix(0x9e37_79b9_7f4a_7c15).to_be_bytes());
    // Canonical v4: the vectors' ids have to be admissible wherever a real one
    // would be, or a fixture would pass a check a real request could not.
    bytes[6] = 0x40 | (bytes[6] & 0x0f);
    bytes[8] = 0x80 | (bytes[8] & 0x3f);
    CloudId::from_bytes(bytes)
}

/// The vectors spell a blob address as `b3:aa` — an address, not a digest.
/// Expand it to the canonical 64 hex characters a real one has, stably.
pub fn fixture_hash(label: &str) -> String {
    let digest = label.strip_prefix("b3:").unwrap_or(label);
    let filler: String = std::iter::repeat_n(digest.chars(), 64)
        .flatten()
        .take(64)
        .collect();
    format!("b3:{filler}")
}

#[cfg(test)]
mod tests;
