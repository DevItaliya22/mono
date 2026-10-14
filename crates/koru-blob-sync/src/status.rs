// SPDX-License-Identifier: MPL-2.0

//! What the engine can be asked about itself (plan §15).
//!
//! *A component nobody can inspect is a component nobody can trust*, and by
//! the time somebody wants to look it is too late to add the seam. Everything
//! here is a handful of integers held in memory and is always live — there is
//! no flag to turn on during the incident.
//!
//! **Ids, counts and codes only.** Never a filename, never a title, never a
//! capability, never a byte. The one string that is not a code is the object
//! host, and it is there because "the upload is stuck" has two very different
//! causes once there are two services.

use koru_store_core::WorkspaceId;

/// Which way bytes were last moving.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Direction {
    Upload,
    Download,
}

/// **Which path a transfer used.** Both arms are live: B8 wired peer-first
/// ordering for a Cloud Workspace, fenced to a genuinely direct path. The
/// variant was named before the arm existed, because a status field that
/// appears with the feature is a status field nobody was reading before it.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum TransferPath {
    Cloud,
    PeerDirect,
}

impl TransferPath {
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Cloud => "cloud",
            Self::PeerDirect => "peer_direct",
        }
    }
}

/// **What is about to move, measured before the first byte does** (D256).
///
/// *Tell, then start.* Enabling Cloud on a Workspace of several years can be
/// gigabytes on the first afternoon, and every other phase of this plan makes
/// that correct without making it comprehensible. This is the sentence a
/// person reads first.
///
/// **It is the reconciliation walk's own answer, not a second walk.** The
/// enable pass walks once, resolves what it found against the local object
/// store once, publishes this, and then starts. `BlobTransferStatus::walks`
/// is the measurement that keeps that true.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct BlobPreflight {
    /// Every live hash the walk found.
    pub files: u32,
    /// Of those, the ones this device holds — the ones about to be uploaded.
    pub upload_files: u32,
    /// **What those weigh**, exactly, because their sizes are local facts.
    pub upload_bytes: u64,
    /// Of those, the ones this device does not hold **and will fetch** — the
    /// ones something asked for (D260). An absent file nobody asked for is
    /// not a download about to happen and is not counted.
    ///
    /// Counted without a size: a blob this device has never seen has no local
    /// size, and the plan's §12 is explicit that a guessed number is worse
    /// than an absent one.
    pub download_files: u32,
    /// References the walk could not turn into an address. Carried here as
    /// well as on the status so the summary cannot claim a completeness the
    /// walk did not have.
    pub unreadable: u32,
    /// When it was taken. Epoch milliseconds, for a surface that renders it.
    pub at_millis: i64,
}

/// One Workspace's blob transfer state, as a person or a route reads it.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct BlobTransferStatus {
    pub workspace: Option<WorkspaceId>,
    /// Live hashes the canonical walk found.
    pub referenced: u32,
    /// Of those, the ones whose bytes this device holds and this Workspace is
    /// associated with.
    pub present: u32,
    /// Of those, the ones Cloud has confirmed durable in this pass.
    pub durable: u32,
    pub uploading: u32,
    pub downloading: u32,
    /// Referenced, asked for, and parked by the last pass — a file that could
    /// not arrive. A file nobody asked for is not here: since D260 it stays
    /// on Cloud and is nothing to wait on.
    pub missing: u32,
    pub bytes_complete: u64,
    pub bytes_total: u64,
    /// The stable code the pass is currently waiting on, or `None` when
    /// nothing is pending. From the contract's §7 registry, never prose.
    pub pending_reason: Option<&'static str>,
    /// The last stable failure code, whatever pass it came from.
    pub last_failure_code: Option<&'static str>,
    pub direction: Option<Direction>,
    pub path: Option<TransferPath>,
    /// **Which service answered.** The object plane's host, with no scheme and
    /// no path — the control plane and the byte plane fail differently and a
    /// person needs to know which one went quiet.
    pub object_host: Option<String>,
    /// When the earliest parked or backed-off blob will be tried again.
    pub oldest_retry_at_millis: Option<i64>,
    /// References the walk could not turn into an address. A coverage number
    /// that hid these would claim a completeness it has not got.
    pub unreadable_references: u32,
    /// How many canonical walks this engine has run, ever.
    ///
    /// A diagnostic and a **test surface**: "typing and switching tabs perform
    /// no full-Workspace walk" is a claim about this number not moving, which
    /// is a measurement rather than an assertion about intent.
    pub walks: u64,
    /// How many reconciliation passes have run, and how many were skipped
    /// because the Workspace's hash set had not moved. The ratio is the answer
    /// to "is this thing walking on every commit".
    pub passes: u64,
    pub passes_skipped: u64,
    /// **The last committed coverage claim**, documents and pictures counted
    /// separately ([`crate::coverage`]).
    ///
    /// `None` is not zero and is not *no*: it means this device has not
    /// computed coverage for the Workspace yet — no Cloud pass has finished on
    /// it since the process started. A surface that read `None` as "nothing is
    /// missing" would claim a completeness it has never measured, which is the
    /// whole failure this field exists to make impossible.
    pub coverage: Option<crate::coverage::CoverageReport>,
    /// **A person pressed pause for this Workspace** (D256).
    ///
    /// Separate from `pending_reason`, which carries whatever the newest pass
    /// stopped on: the pause is durable within this process and outlives the
    /// pass that reported it, so a surface can say *"you paused this"* even
    /// before the next pass has run to say so.
    pub paused: bool,
    /// What the last enable pass found before it started moving anything.
    ///
    /// `None` until Cloud has been enabled for this Workspace in this process
    /// — which is honest, and is not the same as zero.
    pub preflight: Option<BlobPreflight>,
}

impl BlobTransferStatus {
    /// Referenced minus present: what a person is still waiting for.
    pub fn outstanding(&self) -> u32 {
        self.referenced.saturating_sub(self.present)
    }

    /// **Is the Workspace current?** Only a committed, complete coverage claim
    /// says yes.
    ///
    /// Deliberately not derived from `referenced == present`: local presence
    /// is *this device* holding bytes and coverage is *Cloud* holding them. A
    /// Workspace whose every picture sits on this laptop and none of which has
    /// been uploaded has `present == referenced` and is not current.
    pub fn is_current(&self) -> bool {
        self.coverage
            .as_ref()
            .is_some_and(crate::coverage::CoverageReport::is_current)
    }

    /// **This device's own view of the blob half**, for a surface that must
    /// say something before a claim has been committed.
    ///
    /// [`Self::is_current`] is the authority and this is not: it answers from
    /// the counts this pass produced rather than from a committed claim, so it
    /// can read complete in the moment between the last object being verified
    /// and coverage being recomputed. A surface may use it to phrase *"the
    /// pictures are here"*; nothing may use it to decide the Workspace is
    /// current.
    ///
    /// The coverage brief reports document and blob coverage separately, and a surface that
    /// answered "up to date" from the document half alone would be claiming a
    /// completeness it has not got while files are still moving. This is
    /// the blob half of that sentence, and nothing else may be used for it.
    ///
    /// **And never while anything is in flight** (D259). The counts and the
    /// activity indicator were two answers to one question on the surface
    /// that reads this, and a panel that said *sending* beside *up to date*
    /// is a panel a person cannot use. [`Self::in_flight`] is folded in here
    /// rather than left to each caller, because "each caller remembers" is
    /// how the two came apart in the first place.
    pub fn complete(&self) -> bool {
        self.referenced == self.durable && self.unreadable_references == 0 && !self.in_flight()
    }

    /// **Are bytes actually moving for this Workspace right now?**
    ///
    /// Not `direction`: that is the last way bytes went and a pass does not
    /// clear it, so a finished upload still names itself an upload. These two
    /// counts are the ones a pass sets while a transfer is open and leaves at
    /// zero when none is.
    pub fn in_flight(&self) -> bool {
        self.uploading > 0 || self.downloading > 0
    }
}

#[cfg(test)]
mod tests;
