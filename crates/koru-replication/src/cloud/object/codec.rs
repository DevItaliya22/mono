// SPDX-License-Identifier: MPL-2.0

use super::*;
use loro::{Frontiers, VersionVector};

const FIXED_HEADER_BYTES: u32 = 257;
const SUBJECT_BYTES: u32 = 16;

/// `shape` permits two nonempty vectors: result plus predecessor (segment)
/// or result plus frontiers (checkpoint). Include the largest unit identity.
pub(super) fn header_bound(vector: u32) -> Option<u32> {
    vector
        .checked_mul(2)?
        .checked_add(FIXED_HEADER_BYTES + SUBJECT_BYTES)
}

/// Declaration/size probe only. This cannot establish valid Loro content or
/// justify allocating the returned size. No input-sized allocation occurs.
pub fn preflight_length(header: u32, payload: u64, limits: Limits) -> Result<u64, Error> {
    if header > limits.header {
        tracing::warn!(
            check = "object_header",
            actual_bytes = header,
            limit_bytes = limits.header,
            "cloud object: size limit rejected header"
        );
        return Err(Error::TooLarge);
    }
    integer(payload, false)?;
    let total = (PREFIX_BYTES as u64)
        .checked_add(u64::from(header))
        .and_then(|v| v.checked_add(payload))
        .ok_or(Error::Integer)?;
    integer(total, true)?;
    Ok(total)
}

struct Cursor<'a> {
    bytes: &'a [u8],
    position: usize,
}
impl<'a> Cursor<'a> {
    fn take(&mut self, count: usize) -> Result<&'a [u8], Error> {
        let end = self.position.checked_add(count).ok_or(Error::Integer)?;
        let value = self.bytes.get(self.position..end).ok_or(Error::Truncated)?;
        self.position = end;
        Ok(value)
    }
    fn array<const N: usize>(&mut self) -> Result<[u8; N], Error> {
        self.take(N)?.try_into().map_err(|_| Error::Truncated)
    }
    fn byte(&mut self) -> Result<u8, Error> {
        Ok(self.array::<1>()?[0])
    }
    fn u32(&mut self) -> Result<u32, Error> {
        Ok(u32::from_be_bytes(self.array()?))
    }
    fn u64(&mut self) -> Result<u64, Error> {
        Ok(u64::from_be_bytes(self.array()?))
    }
    fn id(&mut self) -> Result<CloudId, Error> {
        Ok(CloudId::from_bytes(self.array()?))
    }
    fn vector(&mut self, limits: Limits) -> Result<&'a [u8], Error> {
        let size = self.u32()?;
        if size > limits.vector {
            tracing::warn!(
                check = "object_decode_vector",
                actual_bytes = size,
                limit_bytes = limits.vector,
                "cloud object: size limit rejected version metadata"
            );
            return Err(Error::TooLarge);
        }
        self.take(usize::try_from(size).map_err(|_| Error::TooLarge)?)
    }
}

fn shape(metadata: Metadata, body: Body<'_>, limits: Limits) -> Result<(), Error> {
    metadata.validate()?;
    limits.payload(metadata.kind, body.payload.len() as u64)?;
    for (field, bytes) in [
        ("predecessor_version", body.predecessor_version),
        ("result_version", body.result_version),
        ("checkpoint_frontiers", body.checkpoint_frontiers),
    ] {
        if bytes.len() as u64 > u64::from(limits.vector) {
            tracing::warn!(
                dataset = ?metadata.dataset,
                unit = ?metadata.unit,
                check = "object_encode_vector",
                field,
                actual_bytes = bytes.len(),
                limit_bytes = limits.vector,
                "cloud object: size limit rejected version metadata"
            );
            return Err(Error::TooLarge);
        }
    }
    if body.result_version.is_empty() {
        return Err(Error::CausalShape);
    }
    match metadata.kind {
        Kind::Segment
            if body.predecessor_version.is_empty() || !body.checkpoint_frontiers.is_empty() =>
        {
            Err(Error::CausalShape)
        }
        Kind::Checkpoint
            if !body.predecessor_version.is_empty() || body.checkpoint_frontiers.is_empty() =>
        {
            Err(Error::CausalShape)
        }
        _ => Ok(()),
    }
}
fn vectors(body: Body<'_>) -> Result<(), Error> {
    VersionVector::decode(body.result_version).map_err(|_| Error::Loro)?;
    if !body.predecessor_version.is_empty() {
        VersionVector::decode(body.predecessor_version).map_err(|_| Error::Loro)?;
    }
    if !body.checkpoint_frontiers.is_empty() {
        Frontiers::decode(body.checkpoint_frontiers).map_err(|_| Error::Loro)?;
    }
    Ok(())
}

/// Borrowed exact-object decoder. The owner must first use an exact size probe
/// and bounded reader when acquiring bytes; this function performs no I/O.
/// All declared lengths/shapes are checked before Loro allocates decoded VVs.
pub fn decode(bytes: &[u8], limits: Limits) -> Result<Decoded<'_>, Error> {
    let mut prefix = Cursor { bytes, position: 0 };
    if prefix.take(8)? != b"KORULR01" {
        return Err(Error::Magic);
    }
    if u16::from_be_bytes(prefix.array()?) != 1 {
        return Err(Error::Version);
    }
    let header_length = prefix.u32()?;
    let payload_length = prefix.u64()?;
    let total = preflight_length(header_length, payload_length, limits)?;
    if total != bytes.len() as u64 {
        return Err(Error::Length);
    }
    let header = prefix.take(usize::try_from(header_length).map_err(|_| Error::TooLarge)?)?;
    let mut cursor = Cursor {
        bytes: header,
        position: 0,
    };
    let kind = match cursor.byte()? {
        1 => Kind::Checkpoint,
        2 => Kind::Segment,
        _ => return Err(Error::Tag),
    };
    let dataset_tag = cursor.byte()?;
    if !matches!(dataset_tag, 1 | 2) {
        return Err(Error::Tag);
    }
    if cursor.byte()? != 1 {
        return Err(Error::Region);
    }
    limits.payload(kind, payload_length)?;
    let unit_tag = cursor.byte()?;
    let authority = cursor.id()?;
    let lineage = cursor.id()?;
    let replica = cursor.id()?;
    let dataset = if dataset_tag == 1 {
        CloudReplicaPrincipalV1::Workspace {
            workspace_id: authority,
            incarnation_id: lineage,
            logical_replica_id: replica,
        }
    } else {
        CloudReplicaPrincipalV1::Profile {
            profile_id: authority,
            lineage_id: lineage,
            logical_replica_id: replica,
        }
    };
    let cloud_unit_id = cursor.id()?;
    let object_id = cursor.id()?;
    let sequence = cursor.u64()?;
    let subject = match cursor.byte()? {
        0 => None,
        1 => Some(cursor.id()?),
        _ => return Err(Error::Unit),
    };
    let unit = Unit::parse(unit_tag, subject)?;
    let writer_worker_id = cursor.id()?;
    let writer_lease_id = cursor.id()?;
    let writer_lease_epoch = cursor.u64()?;
    let authority_workspace_id = cursor.id()?;
    let authorization_version = cursor.u64()?;
    let membership_epoch = cursor.u64()?;
    let cloud_access_epoch = cursor.u64()?;
    let unit_ref_digest: Hash = cursor.array()?;
    if unit.digest(dataset)? != unit_ref_digest {
        return Err(Error::Checksum);
    }
    let predecessor_version = cursor.vector(limits)?;
    let result_version = cursor.vector(limits)?;
    let checkpoint_frontiers = cursor.vector(limits)?;
    let covers_through_sequence = cursor.u64()?;
    let payload_sha256: Hash = cursor.array()?;
    if cursor.position != header.len() {
        return Err(Error::Length);
    }
    let metadata = Metadata {
        kind,
        dataset,
        unit,
        cloud_unit_id,
        object_id,
        sequence,
        writer_worker_id,
        writer_lease_id,
        writer_lease_epoch,
        authority_workspace_id,
        authorization_version,
        membership_epoch,
        cloud_access_epoch,
        covers_through_sequence,
    };
    let body = Body {
        predecessor_version,
        result_version,
        checkpoint_frontiers,
        payload: prefix.take(usize::try_from(payload_length).map_err(|_| Error::TooLarge)?)?,
    };
    shape(metadata, body, limits)?;
    // Byte checks precede Loro parsing; no payload copy or normalization.
    if hash(body.payload) != payload_sha256 {
        return Err(Error::Checksum);
    }
    vectors(body)?;
    let mut request = Sha256::new();
    request.update(b"koru/cloud/loro-object-request/v1\0");
    request.update(header);
    Ok(Decoded {
        metadata,
        body,
        commitments: Commitments {
            unit_ref_digest,
            predecessor_version_digest: (!predecessor_version.is_empty())
                .then(|| hash(predecessor_version)),
            result_version_digest: koru_store_core::sync_digest::revision(result_version),
            checkpoint_frontier_digest: (!checkpoint_frontiers.is_empty())
                .then(|| hash(checkpoint_frontiers)),
            payload_sha256,
            object_sha256: hash(bytes),
            request_digest: request.finalize().into(),
            payload_byte_length: payload_length,
            object_byte_length: total,
        },
    })
}

/// Encodes exactly the caller's captured bytes. This is not a Loro export,
/// publication, or receipt. All lengths precede reservation of the output Vec.
pub fn encode(metadata: Metadata, body: Body<'_>, limits: Limits) -> Result<Vec<u8>, Error> {
    shape(metadata, body, limits)?;
    let vectors_length = [
        body.predecessor_version,
        body.result_version,
        body.checkpoint_frontiers,
    ]
    .into_iter()
    .try_fold(0_usize, |sum, bytes| {
        sum.checked_add(bytes.len()).ok_or(Error::Integer)
    })?;
    // 257 fixed header bytes; subject contributes 16 when present.
    let header_length = (FIXED_HEADER_BYTES as usize)
        .checked_add(if metadata.unit.subject().is_some() {
            SUBJECT_BYTES as usize
        } else {
            0
        })
        .and_then(|n| n.checked_add(vectors_length))
        .ok_or(Error::Integer)?;
    let header_length = u32::try_from(header_length).map_err(|_| Error::TooLarge)?;
    let total = preflight_length(header_length, body.payload.len() as u64, limits)?;
    vectors(body)?;
    let total = usize::try_from(total).map_err(|_| Error::TooLarge)?;
    let mut out = Vec::new();
    out.try_reserve_exact(total)
        .map_err(|_| Error::Allocation)?;
    out.extend_from_slice(b"KORULR01");
    out.extend_from_slice(&1_u16.to_be_bytes());
    out.extend_from_slice(&header_length.to_be_bytes());
    out.extend_from_slice(&(body.payload.len() as u64).to_be_bytes());
    let (dataset_tag, authority, lineage) = identity::parts(metadata.dataset);
    out.extend_from_slice(&[
        if metadata.kind == Kind::Checkpoint {
            1
        } else {
            2
        },
        dataset_tag,
        1,
        metadata.unit.tag(),
    ]);
    for id in [
        authority,
        lineage,
        metadata.dataset.logical_replica_id(),
        metadata.cloud_unit_id,
        metadata.object_id,
    ] {
        out.extend_from_slice(id.as_bytes());
    }
    out.extend_from_slice(&metadata.sequence.to_be_bytes());
    out.push(u8::from(metadata.unit.subject().is_some()));
    if let Some(subject) = metadata.unit.subject() {
        out.extend_from_slice(subject.as_bytes());
    }
    out.extend_from_slice(metadata.writer_worker_id.as_bytes());
    out.extend_from_slice(metadata.writer_lease_id.as_bytes());
    out.extend_from_slice(&metadata.writer_lease_epoch.to_be_bytes());
    out.extend_from_slice(metadata.authority_workspace_id.as_bytes());
    for epoch in [
        metadata.authorization_version,
        metadata.membership_epoch,
        metadata.cloud_access_epoch,
    ] {
        out.extend_from_slice(&epoch.to_be_bytes());
    }
    out.extend_from_slice(&metadata.unit.digest(metadata.dataset)?);
    for vector in [
        body.predecessor_version,
        body.result_version,
        body.checkpoint_frontiers,
    ] {
        out.extend_from_slice(&(vector.len() as u32).to_be_bytes());
        out.extend_from_slice(vector);
    }
    out.extend_from_slice(&metadata.covers_through_sequence.to_be_bytes());
    out.extend_from_slice(&hash(body.payload));
    out.extend_from_slice(body.payload);
    debug_assert_eq!(out.len(), total);
    Ok(out)
}

#[cfg(test)]
mod tests;
