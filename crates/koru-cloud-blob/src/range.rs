// SPDX-License-Identifier: MPL-2.0

//! `Content-Range` admission for `PATCH …/uploads/{upload}`.
//!
//! Pure decision. The worker supplies the current committed offset — the synced
//! length of the staging file, which **is** the contiguous durable prefix — and
//! this says whether the chunk may be appended, is an idempotent replay whose
//! bytes still have to match, or is refused and why.

use crate::{BlobCode, CHUNK};

/// One parsed `bytes <start>-<end>/<total>`. There is no `*` form: a client
/// that does not know the total cannot be sending a bounded chunk of it.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct ContentRange {
    pub start: u64,
    /// Inclusive, exactly as the header spells it.
    pub end: u64,
    pub total: u64,
}

impl ContentRange {
    /// Inclusive end, so the length is one more than the difference. Only ever
    /// called after [`parse_content_range`] has refused an inverted range.
    pub fn length(self) -> u64 {
        self.end - self.start + 1
    }
}

/// What the worker should do with an admitted chunk.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum RangeOutcome {
    /// Append these bytes, then flush and sync **before** acknowledging the
    /// new offset. An acknowledged offset is on the disk.
    Append {
        start: u64,
        length: u64,
        committed_after: u64,
    },
    /// A range wholly inside the durable prefix. Idempotent when the bytes are
    /// identical — which the caller must still check, because only it has them
    /// — and `range_conflict` when they are not. The offset does not move.
    Replay {
        start: u64,
        length: u64,
        committed_after: u64,
    },
}

impl RangeOutcome {
    pub fn committed_after(self) -> u64 {
        match self {
            Self::Append {
                committed_after, ..
            }
            | Self::Replay {
                committed_after, ..
            } => committed_after,
        }
    }
}

/// Strictly `bytes <start>-<end>/<total>`, single ASCII spaces, decimal digits
/// with no sign and no leading `+`. Anything else is unparseable, which is one
/// of the three things §7 calls `malformed_range`.
pub fn parse_content_range(raw: &str) -> Result<ContentRange, BlobCode> {
    let malformed = Err(BlobCode::MalformedRange);
    let Some(rest) = raw.strip_prefix("bytes ") else {
        return malformed;
    };
    let Some((span, total)) = rest.split_once('/') else {
        return malformed;
    };
    let Some((start, end)) = span.split_once('-') else {
        return malformed;
    };
    let number = |value: &str| -> Option<u64> {
        if value.is_empty() || !value.bytes().all(|byte| byte.is_ascii_digit()) {
            return None;
        }
        // A leading zero is a second spelling of one number; refuse it so the
        // header a client sends and the header a proxy logs are the same text.
        if value.len() > 1 && value.starts_with('0') {
            return None;
        }
        value.parse().ok()
    };
    let (Some(start), Some(end), Some(total)) = (number(start), number(end), number(total)) else {
        return malformed;
    };
    Ok(ContentRange { start, end, total })
}

/// Admit one chunk against the session's committed offset and the capability's
/// bound length.
///
/// The order is structural first: a range that is not a range at all cannot be
/// compared against anything. Then the two lengths the client declared, then
/// contiguity — so a client with a stale offset is told `range_conflict` rather
/// than being sent away for a body-length problem it would still have after
/// resyncing.
pub fn admit_range(
    range: ContentRange,
    committed: u64,
    capability_length: u64,
    body_length: u64,
) -> Result<RangeOutcome, BlobCode> {
    if range.end < range.start {
        return Err(BlobCode::MalformedRange);
    }
    let length = range.length();
    if length > CHUNK {
        return Err(BlobCode::MalformedRange);
    }
    // The last byte of the object is `total - 1`; a range past it is not a
    // piece of this object whatever the capability says.
    if range.total == 0 || range.end >= range.total {
        return Err(BlobCode::MalformedRange);
    }
    if range.total != capability_length {
        return Err(BlobCode::LengthMismatch);
    }
    if body_length != length {
        return Err(BlobCode::LengthMismatch);
    }
    if range.start == committed {
        return Ok(RangeOutcome::Append {
            start: range.start,
            length,
            committed_after: range.end + 1,
        });
    }
    // Wholly inside the durable prefix: the bytes are already on the disk and
    // the client is repeating itself, which is what a lost acknowledgement
    // looks like from the other side.
    if range.end + 1 <= committed {
        return Ok(RangeOutcome::Replay {
            start: range.start,
            length,
            committed_after: committed,
        });
    }
    // A gap, or a range straddling the prefix. Either way the worker cannot
    // append it and must not guess which bytes the client meant.
    Err(BlobCode::RangeConflict)
}

#[cfg(test)]
mod tests;
