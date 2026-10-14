// SPDX-License-Identifier: MPL-2.0

//! Every JSON body the contract names, in one place, so the control API, the
//! object worker and the transfer engine cannot disagree about a field name.
//!
//! Each shape is strict in both directions — unknown fields are refused, not
//! ignored — because a field a reader silently drops is a field a writer thinks
//! it sent.

use koru_account_core::workspace_authorization::Region;
use koru_device::cloud::CloudId;
use serde::{Deserialize, Serialize};

use crate::{BlobCode, locator::ObjectLocator};

/// The failure body every blob route answers with, and the same shape the
/// account routes already use: a stable code a client branches on and
/// untranslated developer text it must never show a person.
#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
pub struct BlobErrorV1 {
    pub code: BlobCode,
    pub message: String,
}

impl From<BlobCode> for BlobErrorV1 {
    fn from(code: BlobCode) -> Self {
        Self {
            code,
            message: code.message().into(),
        }
    }
}

/// `GET …/blobs/{hash}` — §3.1.
///
/// `committed_offset` here is **advisory**. The authority is the worker's
/// `HEAD`, because the worker holds the bytes and the API holds a projection of
/// what it was last told.
#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq)]
#[serde(tag = "state", rename_all = "snake_case", deny_unknown_fields)]
pub enum BlobStatusV1 {
    Missing {
        /// Why the most recent attempt at this address ended, when one did.
        ///
        /// §4.3 promises `hash_mismatch` is *visible on status*, and a session
        /// that failed verification is terminal — so the blob is missing and
        /// this is the only place the reason could be shown. Absent when the
        /// address was simply never uploaded.
        #[serde(default, skip_serializing_if = "Option::is_none")]
        failure: Option<BlobCode>,
    },
    Uploading {
        upload_id: CloudId,
        committed_offset: u64,
        chunk_size: u64,
        expires_at_millis: i64,
        byte_url: String,
    },
    Durable {
        size: u64,
        mime: String,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        filename: Option<String>,
        durable_at_millis: i64,
    },
}

/// `POST …/blobs/{hash}/uploads` — §3.2. The size is exact and is refused
/// above the one ceiling **here**, by the declared size, never after a gigabyte
/// has been sent.
#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
pub struct AllocateRequestV1 {
    pub size: u64,
    pub mime: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub filename: Option<String>,
}

/// The answer to an allocation or a renewal.
///
/// Allocation is idempotent by `(workspace_id, incarnation_id, blob_hash)`, so
/// two devices racing on one picture converge on one session and one locator;
/// a hash that is already durable answers `durable` and starts no writer.
#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq)]
#[serde(tag = "state", rename_all = "snake_case", deny_unknown_fields)]
pub enum AllocationV1 {
    Uploading {
        upload_id: CloudId,
        object_locator: ObjectLocator,
        committed_offset: u64,
        chunk_size: u64,
        expires_at_millis: i64,
        byte_url: String,
        capability: String,
    },
    Durable {
        object_locator: ObjectLocator,
        size: u64,
        mime: String,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        filename: Option<String>,
        durable_at_millis: i64,
    },
}

/// `DELETE …/blob-uploads/{upload}` — §3.4. Idempotent: aborting an already
/// terminal session answers the same thing rather than failing, because the
/// client's intent is satisfied either way.
#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
pub struct AbortedV1 {
    pub state: &'static str,
}

impl Default for AbortedV1 {
    fn default() -> Self {
        Self { state: "aborted" }
    }
}

/// `POST …/blobs/{hash}/read-capability` — §3.5. Issued **only** for a durable
/// mapping.
#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
pub struct ReadCapabilityV1 {
    pub capability: String,
    pub byte_url: String,
    pub size: u64,
    pub mime: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub filename: Option<String>,
}

/// `PATCH …/uploads/{upload}` — §4.2. The offset returned is on the disk: the
/// worker flushes and syncs before it says this.
#[derive(Clone, Copy, Debug, Serialize, Deserialize, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
pub struct CommittedOffsetV1 {
    pub committed_offset: u64,
}

/// `POST …/uploads/{upload}/complete` — §4.3. `202`, and the hash runs as its
/// own job off the request path.
#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
pub struct VerifyingV1 {
    pub state: String,
}

impl Default for VerifyingV1 {
    fn default() -> Self {
        Self {
            state: "verifying".into(),
        }
    }
}

/// `POST /v1/internal/object-reports` — §5. The worker tells the control
/// authority that a verified object exists; the API then performs the
/// authoritative read and commits durability in one transaction.
///
/// Idempotent by `upload_id`: a repeated report for an already durable session
/// answers `200` and writes nothing, so the worker may retry freely.
#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
pub struct ObjectReportV1 {
    pub upload_id: CloudId,
    pub object_locator: ObjectLocator,
    pub region: Region,
    pub size: u64,
    pub blob_hash: String,
    pub verified_at_millis: i64,
}

/// What the API answers a report with. `fenced` is not a failure of the worker:
/// authorization moved, the session is terminal, and the object and its staging
/// are reclaimed. Bytes existing without a mapping is an expected state.
#[derive(Clone, Copy, Debug, Serialize, Deserialize, PartialEq, Eq)]
#[serde(tag = "state", rename_all = "snake_case", deny_unknown_fields)]
pub enum ReportOutcomeV1 {
    Durable,
    Fenced { reason: BlobCode },
}

#[cfg(test)]
mod tests;
