// SPDX-License-Identifier: MPL-2.0

//! Binary framing for workspace bundles.
//!
//! The Store API and every client use this one codec.  The bundle's manifest
//! is the index; document snapshots and blobs follow it in manifest order.

use koru_store_core::bundle::{Bundle, BundleManifest};
use koru_store_core::errors::{Code, Error, Result};
use koru_store_core::model::BlobHash;

/// Self-identifying prefix for a framed Koru workspace bundle.
pub const MAGIC: &[u8; 4] = b"KORU";

pub fn encode(bundle: &Bundle) -> Result<Vec<u8>> {
    bundle.validate()?;
    let manifest = serde_json::to_vec(&bundle.manifest)
        .map_err(|e| Error::new(Code::Internal, format!("encode manifest: {e}")))?;
    let manifest_len = u32::try_from(manifest.len())
        .map_err(|_| Error::new(Code::InvalidRequest, "bundle manifest is too large"))?;

    let mut out = Vec::with_capacity(manifest.len() + 64);
    out.extend_from_slice(MAGIC);
    out.extend_from_slice(&manifest_len.to_le_bytes());
    out.extend_from_slice(&manifest);

    for document in &bundle.manifest.documents {
        let bytes = bundle.snapshot_of(document.id).ok_or_else(|| {
            Error::new(
                Code::InvalidRequest,
                "a snapshot named by the manifest is missing",
            )
        })?;
        push_entry(&mut out, bytes)?;
    }
    for blob in &bundle.manifest.blobs {
        let bytes = bundle
            .blobs
            .iter()
            .find(|(hash, _)| *hash == blob.hash)
            .map(|(_, bytes)| bytes.as_slice())
            .ok_or_else(|| {
                Error::new(
                    Code::InvalidRequest,
                    "a blob named by the manifest is missing",
                )
            })?;
        push_entry(&mut out, bytes)?;
    }
    for version in &bundle.manifest.history_versions {
        let bytes = bundle.history_snapshot_of(version.id).ok_or_else(|| {
            Error::new(
                Code::InvalidRequest,
                "a history snapshot named by the manifest is missing",
            )
        })?;
        push_entry(&mut out, bytes)?;
    }
    Ok(out)
}

pub fn decode(bytes: &[u8]) -> Result<Bundle> {
    let mut cursor = Cursor::new(bytes);
    if cursor.take(4)? != MAGIC {
        return Err(Error::new(
            Code::InvalidRequest,
            "this is not a Koru bundle",
        ));
    }
    let manifest_len = cursor.length()?;
    let manifest: BundleManifest = serde_json::from_slice(cursor.take(manifest_len)?)
        .map_err(|e| Error::new(Code::InvalidRequest, format!("bundle manifest: {e}")))?;

    let mut snapshots = Vec::with_capacity(manifest.documents.len());
    for document in &manifest.documents {
        let length = cursor.length()?;
        snapshots.push((document.id, cursor.take(length)?.to_vec()));
    }
    let mut blobs: Vec<(BlobHash, Vec<u8>)> = Vec::with_capacity(manifest.blobs.len());
    for blob in &manifest.blobs {
        let length = cursor.length()?;
        blobs.push((blob.hash.clone(), cursor.take(length)?.to_vec()));
    }
    let mut history_snapshots = Vec::with_capacity(manifest.history_versions.len());
    for version in &manifest.history_versions {
        let length = cursor.length()?;
        history_snapshots.push((version.id, cursor.take(length)?.to_vec()));
    }
    if !cursor.is_finished() {
        return Err(Error::new(
            Code::InvalidRequest,
            "the bundle has trailing bytes",
        ));
    }

    let bundle = Bundle {
        manifest,
        snapshots,
        history_snapshots,
        blobs,
    };
    bundle.validate()?;
    Ok(bundle)
}

fn push_entry(out: &mut Vec<u8>, bytes: &[u8]) -> Result<()> {
    let length = u32::try_from(bytes.len())
        .map_err(|_| Error::new(Code::InvalidRequest, "a bundle entry is too large"))?;
    out.extend_from_slice(&length.to_le_bytes());
    out.extend_from_slice(bytes);
    Ok(())
}

struct Cursor<'a> {
    bytes: &'a [u8],
    at: usize,
}

impl<'a> Cursor<'a> {
    fn new(bytes: &'a [u8]) -> Self {
        Self { bytes, at: 0 }
    }

    fn take(&mut self, n: usize) -> Result<&'a [u8]> {
        let end = self
            .at
            .checked_add(n)
            .filter(|end| *end <= self.bytes.len())
            .ok_or_else(|| {
                Error::new(
                    Code::InvalidRequest,
                    "the bundle ends in the middle of an entry",
                )
            })?;
        let slice = &self.bytes[self.at..end];
        self.at = end;
        Ok(slice)
    }

    fn length(&mut self) -> Result<usize> {
        Ok(u32::from_le_bytes(self.take(4)?.try_into().expect("four bytes")) as usize)
    }

    fn is_finished(&self) -> bool {
        self.at == self.bytes.len()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use koru_store_core::bundle::{BundleDocument, BundleManifest};
    use koru_store_core::ids::{DocumentId, WorkspaceId};
    use koru_store_core::model::{DocumentKind, Timestamp};

    #[test]
    fn round_trips_and_rejects_trailing_bytes() {
        let id = DocumentId::new();
        let workspace = WorkspaceId::new();
        let bytes = koru_store_core::manifest::Manifest::create(
            workspace,
            "Imported",
            Timestamp::from_millis(1),
        )
        .unwrap()
        .export_snapshot()
        .unwrap();
        let bundle = Bundle {
            manifest: BundleManifest {
                format_version: koru_store_core::version::BUNDLE_FORMAT_VERSION,
                workspace,
                name: "Imported".into(),
                created_at: Timestamp::from_millis(1),
                exported_at: Timestamp::from_millis(2),
                lineage: Vec::new(),
                documents: vec![BundleDocument {
                    id,
                    kind: DocumentKind::Manifest,
                    checksum: koru_store_core::lifecycle::checksum(&bytes),
                }],
                history_versions: Vec::new(),
                history_entries: Vec::new(),
                blobs: Vec::new(),
            },
            snapshots: vec![(id, bytes)],
            history_snapshots: Vec::new(),
            blobs: Vec::new(),
        };
        let encoded = encode(&bundle).unwrap();
        assert_eq!(decode(&encoded).unwrap(), bundle);
        let mut extra = encoded;
        extra.push(0);
        assert_eq!(decode(&extra).unwrap_err().code, Code::InvalidRequest);
    }
}
