// SPDX-License-Identifier: MPL-2.0

use super::*;
use koru_account_runtime::identity::SqliteTrustRepository;
use koru_device::TrustRepository;

#[derive(Serialize, Deserialize)]
struct PersistedTrust {
    descriptor: koru_device::DeviceDescriptorV1,
    records: Vec<TrustRecord>,
    effective_revision: u64,
}

/// Only the legacy file has filesystem effects here. Keeping this boundary
/// small lets crash tests use the real SQLite transaction without touching disk.
trait LegacySource {
    fn read(&mut self) -> Result<Option<Vec<u8>>, String>;
    fn archive(&mut self) -> Result<(), String>;
}

struct DiskLegacy<'a>(&'a Path);

pub(super) trait IdentityArchive {
    fn create(&mut self) -> Result<(), String>;
    fn move_if_present(&mut self, name: &str) -> Result<(), String>;
}

pub(super) struct DiskIdentityArchive<'a> {
    pub root: &'a Path,
    pub aside: &'a Path,
}

impl IdentityArchive for DiskIdentityArchive<'_> {
    fn create(&mut self) -> Result<(), String> {
        // A colliding name is a refusal, never permission to overwrite a
        // previous recovery set. Only one direct child is created.
        std::fs::create_dir(self.aside).map_err(|_| "identity_archive_failed".into())
    }
    fn move_if_present(&mut self, name: &str) -> Result<(), String> {
        match std::fs::rename(self.root.join(name), self.aside.join(name)) {
            Ok(()) => Ok(()),
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(()),
            Err(_) => Err("identity_archive_failed".into()),
        }
    }
}

pub(super) fn archive_identity(archive: &mut impl IdentityArchive) -> Result<(), String> {
    archive.create()?;
    // Keep device.json in place until the database and every sidecar have
    // moved. A partial failure cannot mint a new identity on the next open.
    for name in [
        "trust.db-wal",
        "trust.db-shm",
        "trust.db",
        "trust.json",
        "trust.migrated-to-sqlite.json",
        "device.json",
    ] {
        archive.move_if_present(name)?;
    }
    Ok(())
}

impl LegacySource for DiskLegacy<'_> {
    fn read(&mut self) -> Result<Option<Vec<u8>>, String> {
        match std::fs::read(self.0.join("trust.json")) {
            Ok(bytes) => Ok(Some(bytes)),
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(None),
            Err(_) => Err("trust_migration_failed".into()),
        }
    }

    fn archive(&mut self) -> Result<(), String> {
        let source = self.0.join("trust.json");
        let destination = self.0.join("trust.migrated-to-sqlite.json");
        // Never overwrite an earlier recovery copy. An unarchived source is
        // harmless because the committed marker, not file absence, wins.
        if destination.exists() {
            return Err("trust_archive_exists".into());
        }
        std::fs::rename(source, destination).map_err(|_| "trust_archive_failed".into())
    }
}

fn same_identity(a: &koru_device::DeviceDescriptorV1, b: &koru_device::DeviceDescriptorV1) -> bool {
    a.device_id == b.device_id
        && a.signing_public_key == b.signing_public_key
        && a.endpoint_id == b.endpoint_id
}

fn migrate(
    repository: &mut SqliteTrustRepository,
    local: &koru_device::DeviceDescriptorV1,
    source: &mut impl LegacySource,
) -> Result<(), String> {
    if let Some(committed) = repository
        .local_device()
        .map_err(|_| "trust_migration_failed")?
    {
        if !same_identity(&committed, local) || committed.verify().is_err() {
            return Err("device_identity_mismatch".into());
        }
        return Ok(());
    }
    let bytes = source.read()?;
    let legacy = bytes
        .as_ref()
        .map(|bytes| serde_json::from_slice::<PersistedTrust>(bytes))
        .transpose()
        .map_err(|_| "trust_migration_failed")?;
    if legacy
        .as_ref()
        .is_some_and(|legacy| !same_identity(&legacy.descriptor, local))
    {
        return Err("device_identity_mismatch".into());
    }
    let initial = legacy.unwrap_or_else(|| PersistedTrust {
        descriptor: local.clone(),
        records: Vec::new(),
        effective_revision: 0,
    });
    repository
        .import_legacy(
            &initial.descriptor,
            &initial.records,
            initial.effective_revision,
        )
        .map_err(|_| "trust_migration_failed")?;
    if bytes.is_some() && source.archive().is_err() {
        tracing::warn!("legacy trust committed; migration remnant could not be archived");
    }
    Ok(())
}

pub(super) struct TrustState {
    root: PathBuf,
    repository: SqliteTrustRepository,
}

impl TrustState {
    pub(super) fn open(
        root: &Path,
        local: koru_device::DeviceDescriptorV1,
    ) -> Result<Self, String> {
        let mut repository = SqliteTrustRepository::open(&root.join("trust.db"))
            .map_err(|error| error.code.to_owned())?;
        migrate(&mut repository, &local, &mut DiskLegacy(root))?;
        Ok(Self {
            root: root.to_owned(),
            repository,
        })
    }

    pub(super) fn local_descriptor(&self) -> Result<koru_device::DeviceDescriptorV1, String> {
        self.repository
            .local_device()
            .map_err(|error| error.code.to_owned())?
            .ok_or_else(|| "identity_unavailable".to_owned())
    }

    pub(super) fn records(&self) -> Result<Vec<TrustRecord>, String> {
        self.repository
            .records()
            .map_err(|error| error.code.to_owned())
    }

    pub(super) fn revision(&self) -> Result<u64, String> {
        self.repository
            .revision()
            .map_err(|error| error.code.to_owned())
    }

    pub(super) fn commit_pairing(
        &mut self,
        prepared: &koru_pairing::PreparedPairingV1,
    ) -> Result<(), String> {
        let local = self.local_descriptor()?.device_id;
        self.repository
            .commit_pairing(prepared, local)
            .map(|_| ())
            .map_err(|error| error.code.to_owned())
    }

    pub(super) fn apply_direct_trust(
        &mut self,
        from: DeviceId,
        record: TrustRecord,
    ) -> Result<bool, String> {
        // Singular delivery is for an urgent fact from this peer: unlinking
        // this device, or a Profile lifecycle fact it signed. Mesh gossip
        // carries third-party records through the batch path.
        match &record {
            TrustRecord::Revocation(value)
                if value.issued_by == from
                    && value.revoked_device_id == self.local_descriptor()?.device_id => {}
            TrustRecord::ProfileLifecycle(value) if value.issued_by == from => {}
            _ => return Err("unexpected_trust_record".into()),
        }
        let verified = koru_device::verify_gossip_record(&self.view()?, record)
            .map_err(|error| error.to_string())?;
        self.apply_verified(verified)
    }

    /// Apply a gossiped device-list batch. Every record is verified against
    /// this device's own trust view — never against the sender's say-so — in
    /// canonical order, so a batch is verifiable in one pass. A record that
    /// fails is skipped rather than poisoning the rest; the sender's
    /// anti-entropy beat re-tries it.
    ///
    /// Returns whether anything was new. Callers use that to decide whether
    /// to republish authorization: replacing the snapshot closes every live
    /// connection, so doing it for an all-duplicates batch is churn, not
    /// caution.
    pub(super) fn apply_remote_records(
        &mut self,
        mut records: Vec<TrustRecord>,
    ) -> Result<bool, String> {
        koru_device::sort_for_gossip(&mut records);
        let mut rejected = 0usize;
        let mut skipped = 0usize;
        let mut changed = false;
        for record in records {
            // Re-read per record: an applied relationship is what the next
            // grant or revocation in the same batch verifies against.
            match koru_device::verify_gossip_record(&self.view()?, record) {
                Ok(verified) => match self.apply_verified(verified) {
                    Ok(inserted) => changed |= inserted,
                    Err(_) => rejected += 1,
                },
                // A kind this build does not know comes from a newer device.
                // Skipped, not rejected: the batch is still acknowledged, or
                // the sender would offer it again forever.
                Err(koru_device::GossipRejection::UnknownKind) => skipped += 1,
                Err(_) => rejected += 1,
            }
        }
        if skipped > 0 {
            tracing::warn!(
                skipped,
                "trust gossip carried record kinds this build does not know; update this device"
            );
        }
        if rejected == 0 {
            Ok(changed)
        } else {
            Err("rejected_trust_records".into())
        }
    }

    fn apply_verified(
        &mut self,
        verified: koru_device::VerifiedTrustRecord,
    ) -> Result<bool, String> {
        self.repository
            .apply(verified)
            .map(|outcome| matches!(outcome, koru_device::ApplyOutcome::Applied { .. }))
            .map_err(|error| error.code.to_owned())
    }

    /// Describe this device's build: a self-signed `DeviceBuildV1`, written
    /// only when the latest one this device signed says something else, so
    /// a launch on an unchanged build writes nothing.
    pub(super) fn advertise_build(
        &mut self,
        payload: &koru_device::BuildPayload,
        signing_key: &koru_device::DeviceSigningKey,
    ) -> Result<bool, String> {
        let view = self.view()?;
        let device_id = self.local_descriptor()?.device_id;
        if view.build_is_current(device_id, payload) {
            return Ok(false);
        }
        let record = koru_device::DeviceBuildV1::sign(
            koru_device::RecordId::new(),
            device_id,
            payload,
            now_millis(),
            signing_key,
        );
        let verified = koru_device::verify_gossip_record(&view, TrustRecord::DeviceBuild(record))
            .map_err(|error| error.to_string())?;
        self.apply_verified(verified)
    }

    pub(super) fn revoke(
        &mut self,
        peer: DeviceId,
        signing_key: &koru_device::DeviceSigningKey,
    ) -> Result<Vec<TrustRecord>, String> {
        let view = self.view()?;
        let local = view.local_device.device_id;
        let revoked = view
            .revocations
            .iter()
            .map(|record| record.relationship_id)
            .collect::<std::collections::HashSet<_>>();
        let relationships = view
            .relationships
            .iter()
            .filter(|relationship| {
                !revoked.contains(&relationship.relationship_id)
                    && ((relationship.inviter.device_id == local
                        && relationship.joiner.device_id == peer)
                        || (relationship.joiner.device_id == local
                            && relationship.inviter.device_id == peer))
            })
            .map(|relationship| relationship.relationship_id)
            .collect::<Vec<_>>();
        if relationships.is_empty() {
            return Err("device_not_linked".into());
        }
        let mut records = Vec::new();
        for relationship_id in relationships {
            let revocation = koru_device::RevocationV1::sign(
                relationship_id,
                peer,
                local,
                koru_device::RecordId::new(),
                now_millis(),
                signing_key,
            );
            revocation
                .clone()
                .verify(view.local_device.signing_public_key)
                .map_err(|error| error.to_string())?;
            let record = TrustRecord::Revocation(revocation);
            let verified = koru_device::verify_gossip_record(&self.view()?, record.clone())
                .map_err(|error| error.to_string())?;
            self.apply_verified(verified)?;
            records.push(record);
        }
        Ok(records)
    }

    pub(super) fn view(&self) -> Result<koru_device::TrustView, String> {
        self.repository
            .trust_view()
            .map_err(|error| error.code.to_owned())
    }

    /// Rename this device: a self-signed `DeviceNameV1` record, persisted and
    /// gossiped like any other, plus the local descriptor so the *next*
    /// pairing offers the current name.
    pub(super) fn rename_local(
        &mut self,
        display_name: String,
        signing_key: &koru_device::DeviceSigningKey,
    ) -> Result<(), String> {
        let device_id = self.local_descriptor()?.device_id;
        let record = koru_device::DeviceNameV1::sign(
            koru_device::RecordId::new(),
            device_id,
            display_name.clone(),
            now_millis(),
            signing_key,
        );
        let verified =
            koru_device::verify_gossip_record(&self.view()?, TrustRecord::DeviceName(record))
                .map_err(|error| error.to_string())?;
        self.apply_verified(verified)?;
        self.repository
            .set_local_display_name(&display_name)
            .map_err(|error| error.code.to_owned())?;
        // Keep device.json's copy in step: the startup identity check reads
        // it, and the next pairing offer takes this descriptor from it.
        let device_path = self.root.join("device.json");
        let bytes =
            serde_json::to_vec_pretty(&self.local_descriptor()?).map_err(|e| e.to_string())?;
        std::fs::write(device_path, bytes).map_err(|error| error.to_string())
    }
}

#[cfg(test)]
mod tests;
