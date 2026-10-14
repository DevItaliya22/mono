// SPDX-License-Identifier: MPL-2.0

//! Durable public device identity and signed trust facts.
//!
//! Platform hosts provide private keys through their own SecretStore adapters.
//! SQLite schema and pairing/enrollment transaction semantics are shared.

use std::collections::BTreeMap;
use std::path::Path;

use koru_device::{
    ApplyOutcome, DeviceDescriptorV1, DeviceError, DeviceId, TrustRecord, TrustRepository,
    TrustView, VerifiedTrustRecord,
};
use rusqlite::{Connection, OptionalExtension, params};

mod directory;
pub use directory::*;

const DB_VERSION: i64 = 5;

pub struct SqliteTrustRepository {
    connection: Connection,
}

impl SqliteTrustRepository {
    pub fn open(path: &Path) -> Result<Self, DeviceError> {
        if let Some(parent) = path.parent() {
            std::fs::create_dir_all(parent).map_err(io_error)?;
        }
        let connection = Connection::open(path).map_err(db_error)?;
        Self::from_connection(connection)
    }

    #[cfg(any(test, feature = "test-support"))]
    pub fn in_memory() -> Result<Self, DeviceError> {
        Self::from_connection(Connection::open_in_memory().map_err(db_error)?)
    }

    fn from_connection(connection: Connection) -> Result<Self, DeviceError> {
        connection
            .busy_timeout(std::time::Duration::from_secs(5))
            .map_err(db_error)?;
        connection
            .execute_batch(koru_sqlite::DURABILITY)
            .map_err(db_error)?;
        connection
            .pragma_update(None, "foreign_keys", "ON")
            .map_err(db_error)?;
        let version: i64 = connection
            .pragma_query_value(None, "user_version", |row| row.get(0))
            .map_err(db_error)?;
        if version == 0 {
            connection
                .execute_batch(
                    "BEGIN IMMEDIATE;
                     CREATE TABLE local_device (singleton INTEGER PRIMARY KEY CHECK(singleton = 1), descriptor_json BLOB NOT NULL);
                     CREATE TABLE trust_records (record_id TEXT PRIMARY KEY, kind TEXT NOT NULL, record_json BLOB NOT NULL);
                     CREATE TABLE trust_meta (singleton INTEGER PRIMARY KEY CHECK(singleton = 1), effective_revision INTEGER NOT NULL);
                     CREATE TABLE pairing_commits (
                        invitation_id TEXT PRIMARY KEY,
                        peer_device_id TEXT NOT NULL,
                        state TEXT NOT NULL CHECK(state IN ('prepared', 'committed')),
                        prepared_json BLOB NOT NULL
                     );
                     CREATE TABLE enrollment_commits (
                        attestation_jti TEXT PRIMARY KEY,
                        peer_device_id TEXT NOT NULL,
                        prepared_json BLOB NOT NULL
                     );
                     CREATE TABLE enrollment_target (
                        singleton INTEGER PRIMARY KEY CHECK(singleton = 1),
                        profile_id TEXT NOT NULL
                     );
                     INSERT INTO trust_meta(singleton, effective_revision) VALUES(1, 0);
                     PRAGMA user_version = 4;
                     COMMIT;",
                )
                .map_err(db_error)?;
        }
        {
            let mut current = if version == 0 { 4 } else { version };
            if current == 1 {
                connection
                    .execute_batch(
                        "BEGIN IMMEDIATE;
                         CREATE TABLE pairing_commits (
                            invitation_id TEXT PRIMARY KEY,
                            peer_device_id TEXT NOT NULL,
                            state TEXT NOT NULL CHECK(state IN ('prepared', 'committed')),
                            prepared_json BLOB NOT NULL
                         );
                         PRAGMA user_version = 2;
                         COMMIT;",
                    )
                    .map_err(db_error)?;
                current = 2;
            }
            if current == 2 {
                connection
                    .execute_batch(include_str!(
                        "../migrations/007_agent_enrollment_commits.sql"
                    ))
                    .map_err(db_error)?;
                current = 3;
            }
            if current == 3 {
                connection
                    .execute_batch(include_str!(
                        "../migrations/008_agent_enrollment_target.sql"
                    ))
                    .map_err(db_error)?;
                current = 4;
            }
            if current == 4 {
                connection
                    .execute_batch(include_str!(
                        "../migrations/017_account_directory_migration.sql"
                    ))
                    .map_err(db_error)?;
                current = 5;
            }
            if current != DB_VERSION {
                return Err(DeviceError::new(
                    "identity_schema",
                    format!("unsupported agent identity database version {version}"),
                ));
            }
        }
        Ok(Self { connection })
    }

    pub fn revision(&self) -> Result<u64, DeviceError> {
        self.connection
            .query_row(
                "SELECT effective_revision FROM trust_meta WHERE singleton = 1",
                [],
                |row| row.get(0),
            )
            .map_err(db_error)
    }

    /// One-time import into a dedicated phone database. The local-device row
    /// is the committed initialization marker; callers must check it before
    /// reading legacy input. This never overwrites an initialized repository.
    pub fn import_legacy(
        &mut self,
        identity: &DeviceDescriptorV1,
        records: &[TrustRecord],
        effective_revision: u64,
    ) -> Result<(), DeviceError> {
        identity.verify().map_err(|_| legacy_error())?;
        let revision = i64::try_from(effective_revision).map_err(|_| legacy_error())?;
        let transaction = self
            .connection
            .transaction_with_behavior(rusqlite::TransactionBehavior::Immediate)
            .map_err(db_error)?;
        let occupied: bool = transaction
            .query_row(
                "SELECT EXISTS(SELECT 1 FROM local_device) OR EXISTS(SELECT 1 FROM trust_records)
             OR EXISTS(SELECT 1 FROM pairing_commits) OR EXISTS(SELECT 1 FROM enrollment_commits)
             OR EXISTS(SELECT 1 FROM enrollment_unattributed)
             OR EXISTS(SELECT 1 FROM account_namespace) OR
             EXISTS(SELECT 1 FROM trust_meta WHERE effective_revision != 0)",
                [],
                |row| row.get(0),
            )
            .map_err(db_error)?;
        if occupied {
            return Err(legacy_error());
        }
        transaction
            .execute(
                "INSERT INTO local_device(singleton, descriptor_json) VALUES(1, ?1)",
                params![serde_json::to_vec(identity).map_err(json_error)?],
            )
            .map_err(db_error)?;
        for record in records {
            let kind = record_kind(record)?;
            transaction
                .execute(
                    "INSERT INTO trust_records(record_id, kind, record_json) VALUES(?1, ?2, ?3)",
                    params![
                        record.record_id().to_storage(),
                        kind,
                        serde_json::to_vec(record).map_err(json_error)?
                    ],
                )
                .map_err(|_| legacy_error())?;
        }
        transaction
            .execute(
                "UPDATE trust_meta SET effective_revision = ?1 WHERE singleton = 1",
                params![revision],
            )
            .map_err(db_error)?;
        let view = read_trust_view(&transaction)?;
        for record in records {
            // A historical lifecycle fact remains signed evidence after its
            // author is revoked. Admission was checked when received; import
            // checks its signature, not today's authorization to send it.
            if let TrustRecord::ProfileLifecycle(value) = record {
                let issuer = view
                    .descriptor_of(value.issued_by)
                    .ok_or_else(legacy_error)?;
                value
                    .clone()
                    .verify(issuer.signing_public_key)
                    .map_err(|_| legacy_error())?;
            } else {
                koru_device::verify_gossip_record(&view, record.clone())
                    .map_err(|_| legacy_error())?;
            }
        }
        transaction.commit().map_err(db_error)
    }

    pub fn enrollment_target(&self) -> Result<Option<koru_store_core::ProfileId>, DeviceError> {
        let value: Option<String> = self
            .connection
            .query_row(
                "SELECT profile_id FROM enrollment_unattributed WHERE singleton = 1",
                [],
                |row| row.get(0),
            )
            .optional()
            .map_err(db_error)?;
        value
            .map(|value| {
                koru_store_core::ProfileId::parse(&value).ok_or_else(|| {
                    DeviceError::new(
                        "invalid_enrollment_target",
                        "stored enrollment target is not a Profile id",
                    )
                })
            })
            .transpose()
    }

    pub fn set_enrollment_target(
        &mut self,
        profile: Option<koru_store_core::ProfileId>,
    ) -> Result<(), DeviceError> {
        match profile {
            Some(profile) => self
                .connection
                .execute(
                    "INSERT INTO enrollment_unattributed(singleton, profile_id) VALUES(1, ?1)
                     ON CONFLICT(singleton) DO UPDATE SET profile_id = excluded.profile_id",
                    params![profile.to_storage()],
                )
                .map(|_| ())
                .map_err(db_error),
            None => self
                .connection
                .execute(
                    "DELETE FROM enrollment_unattributed WHERE singleton = 1",
                    [],
                )
                .map(|_| ())
                .map_err(db_error),
        }
    }
    /// Every stored trust record in canonical gossip order — the payload for
    /// replicating the device list itself to the mesh.
    pub fn records(&self) -> Result<Vec<TrustRecord>, DeviceError> {
        let mut statement = self
            .connection
            .prepare("SELECT record_json FROM trust_records")
            .map_err(db_error)?;
        let rows = statement
            .query_map([], |row| row.get::<_, Vec<u8>>(0))
            .map_err(db_error)?;
        let mut records = Vec::new();
        for row in rows {
            records.push(serde_json::from_slice(&row.map_err(db_error)?).map_err(json_error)?);
        }
        koru_device::sort_for_gossip(&mut records);
        Ok(records)
    }

    /// Update the local descriptor's display name after a rename, so the
    /// *next* pairing this device offers carries the current name. The
    /// endpoint binding signature does not cover the name, so the descriptor
    /// stays valid.
    pub fn set_local_display_name(&mut self, display_name: &str) -> Result<(), DeviceError> {
        let mut descriptor = self.local_device()?.ok_or_else(|| {
            DeviceError::new("identity_unavailable", "local device is not initialized")
        })?;
        descriptor.display_name = display_name.to_owned();
        if descriptor.verify().is_err() {
            return Err(DeviceError::new(
                "invalid_identity",
                "the renamed descriptor no longer verifies",
            ));
        }
        let bytes = serde_json::to_vec(&descriptor).map_err(json_error)?;
        self.connection
            .execute(
                "UPDATE local_device SET descriptor_json = ?1 WHERE singleton = 1",
                params![bytes],
            )
            .map_err(db_error)?;
        Ok(())
    }

    /// Persist inactive, mutually signed evidence. Authorization deliberately
    /// reads only `trust_records`, so a crash here cannot expose Store data.
    pub fn prepare_pairing(
        &mut self,
        prepared: &koru_pairing::PreparedPairingV1,
        local_device_id: DeviceId,
    ) -> Result<PairingPersistenceState, DeviceError> {
        prepared
            .verify()
            .map_err(|_| DeviceError::new("invalid_pairing", "pairing evidence did not verify"))?;
        let peer = pairing_peer(prepared, local_device_id)?;
        let invitation_id = prepared.transcript.invitation_id.to_storage();
        let bytes = serde_json::to_vec(prepared).map_err(json_error)?;
        let existing: Option<(String, Vec<u8>)> = self
            .connection
            .query_row(
                "SELECT state, prepared_json FROM pairing_commits WHERE invitation_id = ?1",
                params![invitation_id],
                |row| Ok((row.get(0)?, row.get(1)?)),
            )
            .optional()
            .map_err(db_error)?;
        if let Some((state, existing)) = existing {
            if existing != bytes {
                return Err(DeviceError::new(
                    "pairing_collision",
                    "this invitation already names different signed evidence",
                ));
            }
            return Ok(if state == "committed" {
                PairingPersistenceState::Committed
            } else {
                PairingPersistenceState::Prepared
            });
        }
        self.connection
            .execute(
                "INSERT INTO pairing_commits(invitation_id, peer_device_id, state, prepared_json)
                 VALUES(?1, ?2, 'prepared', ?3)",
                params![invitation_id, peer.to_storage(), bytes],
            )
            .map_err(db_error)?;
        Ok(PairingPersistenceState::Prepared)
    }

    /// Atomically publish the relationship and both grants after the remote
    /// durable-prepare receipt has arrived. Replays return `Committed` without
    /// advancing the authorization generation again.
    pub fn commit_pairing(
        &mut self,
        prepared: &koru_pairing::PreparedPairingV1,
        local_device_id: DeviceId,
    ) -> Result<PairingPersistenceState, DeviceError> {
        let state = self.prepare_pairing(prepared, local_device_id)?;
        if state == PairingPersistenceState::Committed {
            return Ok(state);
        }
        let records = [
            TrustRecord::Relationship(prepared.relationship.clone()),
            TrustRecord::AllDataGrant(prepared.inviter_grant.clone()),
            TrustRecord::AllDataGrant(prepared.joiner_grant.clone()),
        ];
        let transaction = self.connection.transaction().map_err(db_error)?;
        let mut inserted = 0u64;
        for record in records {
            let record_id = record.record_id().to_storage();
            let kind = match record {
                TrustRecord::Relationship(_) => "relationship",
                TrustRecord::AllDataGrant(_) => "all_data_grant",
                _ => unreachable!("a pairing only activates relationship and grant records"),
            };
            let bytes = serde_json::to_vec(&record).map_err(json_error)?;
            let existing: Option<Vec<u8>> = transaction
                .query_row(
                    "SELECT record_json FROM trust_records WHERE record_id = ?1",
                    params![record_id],
                    |row| row.get(0),
                )
                .optional()
                .map_err(db_error)?;
            if let Some(existing) = existing {
                if existing != bytes {
                    return Err(DeviceError::new(
                        "record_collision",
                        "a pairing record id already contains a different signed fact",
                    ));
                }
            } else {
                transaction
                    .execute(
                        "INSERT INTO trust_records(record_id, kind, record_json) VALUES(?1, ?2, ?3)",
                        params![record_id, kind, bytes],
                    )
                    .map_err(db_error)?;
                inserted += 1;
            }
        }
        transaction
            .execute(
                "UPDATE pairing_commits SET state = 'committed' WHERE invitation_id = ?1",
                params![prepared.transcript.invitation_id.to_storage()],
            )
            .map_err(db_error)?;
        if inserted != 0 {
            transaction
                .execute(
                    "UPDATE trust_meta SET effective_revision = effective_revision + 1 WHERE singleton = 1",
                    [],
                )
                .map_err(db_error)?;
        }
        transaction.commit().map_err(db_error)?;
        Ok(PairingPersistenceState::Committed)
    }

    /// Return exact previously committed evidence for responder replay.
    pub fn enrollment_by_jti(
        &self,
        attestation_jti: &str,
    ) -> Result<Option<koru_pairing::PreparedEnrollmentV1>, DeviceError> {
        let bytes: Option<Vec<u8>> = self
            .connection
            .query_row(
                "SELECT prepared_json FROM enrollment_commits WHERE attestation_jti = ?1",
                params![attestation_jti],
                |row| row.get(0),
            )
            .optional()
            .map_err(db_error)?;
        bytes
            .map(|bytes| serde_json::from_slice(&bytes).map_err(json_error))
            .transpose()
    }

    /// Rehydrate the latest locally durable completion for one selected Profile.
    pub fn completed_enrollment(
        &self,
        profile: koru_store_core::ProfileId,
        local_device: DeviceId,
    ) -> Result<Option<(koru_pairing::PreparedEnrollmentV1, u64)>, DeviceError> {
        let view = self.trust_view()?;
        let revision = view.effective_revision;
        let mut statement = self
            .connection
            .prepare("SELECT prepared_json FROM enrollment_commits ORDER BY rowid DESC")
            .map_err(db_error)?;
        let mut rows = statement.query([]).map_err(db_error)?;
        while let Some(row) = rows.next().map_err(db_error)? {
            let bytes: Vec<u8> = row.get(0).map_err(db_error)?;
            let prepared: koru_pairing::PreparedEnrollmentV1 =
                serde_json::from_slice(&bytes).map_err(json_error)?;
            if enrollment_completion_is_effective(&prepared, &view, profile, local_device) {
                return Ok(Some((prepared, revision)));
            }
        }
        Ok(None)
    }

    /// Atomically publish one attested relationship and its reciprocal Profile
    /// grants. The attestation JTI is the replay key: identical completion is
    /// idempotent, while reusing it for different signed evidence fails closed.
    pub fn commit_enrollment(
        &mut self,
        prepared: &koru_pairing::PreparedEnrollmentV1,
        attestation: &koru_account_session::VerifiedDeviceAttestationV1,
        local_device_id: DeviceId,
    ) -> Result<ApplyOutcome, DeviceError> {
        prepared.verify(attestation).map_err(|_| {
            DeviceError::new("invalid_enrollment", "enrollment evidence did not verify")
        })?;
        let peer = enrollment_peer(prepared, local_device_id)?;
        let prepared_bytes = serde_json::to_vec(prepared).map_err(json_error)?;
        let transaction = self
            .connection
            .transaction_with_behavior(rusqlite::TransactionBehavior::Immediate)
            .map_err(db_error)?;
        let existing: Option<Vec<u8>> = transaction
            .query_row(
                "SELECT prepared_json FROM enrollment_commits WHERE attestation_jti = ?1",
                params![prepared.transcript.attestation_jti],
                |row| row.get(0),
            )
            .optional()
            .map_err(db_error)?;
        if let Some(existing) = existing {
            if existing != prepared_bytes {
                return Err(DeviceError::new(
                    "enrollment_collision",
                    "this attestation was already consumed by different signed evidence",
                ));
            }
            let revision = transaction
                .query_row(
                    "SELECT effective_revision FROM trust_meta WHERE singleton = 1",
                    [],
                    |row| row.get(0),
                )
                .map_err(db_error)?;
            transaction.commit().map_err(db_error)?;
            return Ok(ApplyOutcome::AlreadyApplied {
                effective_revision: revision,
            });
        }
        let records = [
            TrustRecord::Relationship(prepared.relationship.clone()),
            TrustRecord::ProfileGrant(prepared.existing_grant.clone()),
            TrustRecord::ProfileGrant(prepared.joining_grant.clone()),
        ];
        for record in records {
            let record_id = record.record_id().to_storage();
            let kind = match record {
                TrustRecord::Relationship(_) => "relationship",
                TrustRecord::ProfileGrant(_) => "profile_grant",
                _ => unreachable!("an enrollment has one relationship and Profile grants"),
            };
            let bytes = serde_json::to_vec(&record).map_err(json_error)?;
            let existing: Option<Vec<u8>> = transaction
                .query_row(
                    "SELECT record_json FROM trust_records WHERE record_id = ?1",
                    params![record_id],
                    |row| row.get(0),
                )
                .optional()
                .map_err(db_error)?;
            if let Some(existing) = existing {
                if existing != bytes {
                    return Err(DeviceError::new(
                        "record_collision",
                        "an enrollment record id already contains a different signed fact",
                    ));
                }
            } else {
                transaction
                    .execute(
                        "INSERT INTO trust_records(record_id, kind, record_json) VALUES(?1, ?2, ?3)",
                        params![record_id, kind, bytes],
                    )
                    .map_err(db_error)?;
            }
        }
        transaction
            .execute(
                "INSERT INTO enrollment_commits(attestation_jti, peer_device_id, prepared_json)
                 VALUES(?1, ?2, ?3)",
                params![
                    prepared.transcript.attestation_jti,
                    peer.to_storage(),
                    prepared_bytes
                ],
            )
            .map_err(db_error)?;
        transaction
            .execute(
                "UPDATE trust_meta SET effective_revision = effective_revision + 1 WHERE singleton = 1",
                [],
            )
            .map_err(db_error)?;
        let revision = transaction
            .query_row(
                "SELECT effective_revision FROM trust_meta WHERE singleton = 1",
                [],
                |row| row.get(0),
            )
            .map_err(db_error)?;
        transaction.commit().map_err(db_error)?;
        Ok(ApplyOutcome::Applied {
            effective_revision: revision,
        })
    }
}

#[derive(Copy, Clone, Debug, PartialEq, Eq)]
pub enum PairingPersistenceState {
    Prepared,
    Committed,
}

fn pairing_peer(
    prepared: &koru_pairing::PreparedPairingV1,
    local: DeviceId,
) -> Result<DeviceId, DeviceError> {
    if prepared.transcript.inviter.device_id == local {
        Ok(prepared.transcript.joiner.device_id)
    } else if prepared.transcript.joiner.device_id == local {
        Ok(prepared.transcript.inviter.device_id)
    } else {
        Err(DeviceError::new(
            "invalid_pairing",
            "local device is not a participant in the pairing transcript",
        ))
    }
}

fn enrollment_completion_is_effective(
    prepared: &koru_pairing::PreparedEnrollmentV1,
    view: &TrustView,
    profile: koru_store_core::ProfileId,
    local_device: DeviceId,
) -> bool {
    let relationship = prepared.relationship.relationship_id;
    prepared.transcript.profile_id == profile
        && (prepared.transcript.existing_device.device_id == local_device
            || prepared.transcript.joining_device.device_id == local_device)
        && view
            .relationships
            .iter()
            .any(|candidate| candidate == &prepared.relationship)
        && !view
            .revocations
            .iter()
            .any(|revocation| revocation.relationship_id == relationship)
        && view
            .profile_grants
            .iter()
            .any(|grant| grant == &prepared.existing_grant)
        && view
            .profile_grants
            .iter()
            .any(|grant| grant == &prepared.joining_grant)
}
fn enrollment_peer(
    prepared: &koru_pairing::PreparedEnrollmentV1,
    local: DeviceId,
) -> Result<DeviceId, DeviceError> {
    if prepared.transcript.existing_device.device_id == local {
        Ok(prepared.transcript.joining_device.device_id)
    } else if prepared.transcript.joining_device.device_id == local {
        Ok(prepared.transcript.existing_device.device_id)
    } else {
        Err(DeviceError::new(
            "invalid_enrollment",
            "local device is not a participant in the enrollment transcript",
        ))
    }
}

impl TrustRepository for SqliteTrustRepository {
    fn local_device(&self) -> Result<Option<DeviceDescriptorV1>, DeviceError> {
        read_local_device(&self.connection)
    }

    fn initialize(&mut self, identity: &DeviceDescriptorV1) -> Result<(), DeviceError> {
        identity
            .verify()
            .map_err(|e| DeviceError::new("invalid_identity", e.to_string()))?;
        let bytes = serde_json::to_vec(identity).map_err(json_error)?;
        self.connection
            .execute(
                "INSERT INTO local_device(singleton, descriptor_json) VALUES(1, ?1)",
                params![bytes],
            )
            .map_err(db_error)?;
        Ok(())
    }

    fn apply(&mut self, verified: VerifiedTrustRecord) -> Result<ApplyOutcome, DeviceError> {
        let record = verified.into_record();
        let record_id = record.record_id().to_storage();
        let kind = record_kind(&record)?;
        let bytes = serde_json::to_vec(&record).map_err(json_error)?;
        // Serialize lifecycle comparisons with insertion. Two repository
        // handles may receive the same Profile revision concurrently; neither
        // may decide against a stale snapshot and then publish a conflict.
        let transaction = self
            .connection
            .transaction_with_behavior(rusqlite::TransactionBehavior::Immediate)
            .map_err(db_error)?;
        let existing: Option<Vec<u8>> = transaction
            .query_row(
                "SELECT record_json FROM trust_records WHERE record_id = ?1",
                params![record_id],
                |row| row.get(0),
            )
            .optional()
            .map_err(db_error)?;
        if let Some(existing) = existing {
            let revision = transaction
                .query_row(
                    "SELECT effective_revision FROM trust_meta WHERE singleton = 1",
                    [],
                    |row| row.get(0),
                )
                .map_err(db_error)?;
            if existing == bytes {
                transaction.commit().map_err(db_error)?;
                return Ok(ApplyOutcome::AlreadyApplied {
                    effective_revision: revision,
                });
            }
            return Err(DeviceError::new(
                "record_collision",
                "a different signed fact already uses this record id",
            ));
        }
        if let TrustRecord::ProfileLifecycle(incoming) = &record {
            let current = current_profile_lifecycle(&transaction, incoming.profile_id)?;
            match koru_device::compare_profile_lifecycle(current.as_ref(), incoming) {
                koru_device::ProfileLifecycleOrder::Apply => {}
                koru_device::ProfileLifecycleOrder::Duplicate => {
                    let revision = transaction
                        .query_row(
                            "SELECT effective_revision FROM trust_meta WHERE singleton = 1",
                            [],
                            |row| row.get(0),
                        )
                        .map_err(db_error)?;
                    transaction.commit().map_err(db_error)?;
                    return Ok(ApplyOutcome::AlreadyApplied {
                        effective_revision: revision,
                    });
                }
                koru_device::ProfileLifecycleOrder::Stale => {
                    let revision = transaction
                        .query_row(
                            "SELECT effective_revision FROM trust_meta WHERE singleton = 1",
                            [],
                            |row| row.get(0),
                        )
                        .map_err(db_error)?;
                    transaction.commit().map_err(db_error)?;
                    return Ok(ApplyOutcome::Stale {
                        effective_revision: revision,
                    });
                }
                koru_device::ProfileLifecycleOrder::Conflict => {
                    return Err(DeviceError::new(
                        "profile_lifecycle_conflict",
                        "equal hosted Profile lifecycle revisions disagree",
                    ));
                }
            }
        }
        transaction
            .execute(
                "INSERT INTO trust_records(record_id, kind, record_json) VALUES(?1, ?2, ?3)",
                params![record_id, kind, bytes],
            )
            .map_err(db_error)?;
        if let TrustRecord::DeviceBuild(build) = &record {
            // A device describes its build on every change, where it renames
            // itself rarely — so the superseded description is dropped, or
            // every nightly leaves a row behind on every peer.
            prune_superseded_builds(&transaction, build)?;
        }
        if let TrustRecord::ProfileLifecycle(lifecycle) = &record {
            prune_superseded_lifecycles(&transaction, lifecycle)?;
        }
        transaction.execute("UPDATE trust_meta SET effective_revision = effective_revision + 1 WHERE singleton = 1", []).map_err(db_error)?;
        let revision = transaction
            .query_row(
                "SELECT effective_revision FROM trust_meta WHERE singleton = 1",
                [],
                |row| row.get(0),
            )
            .map_err(db_error)?;
        transaction.commit().map_err(db_error)?;
        Ok(ApplyOutcome::Applied {
            effective_revision: revision,
        })
    }

    fn trust_view(&self) -> Result<TrustView, DeviceError> {
        // Enrollment can commit through another handle to this same database.
        // Descriptor, facts and revision must describe one SQLite snapshot.
        let transaction = self.connection.unchecked_transaction().map_err(db_error)?;
        let view = read_trust_view(&transaction)?;
        transaction.commit().map_err(db_error)?;
        Ok(view)
    }
}

fn read_trust_view(connection: &Connection) -> Result<TrustView, DeviceError> {
    let local_device = read_local_device(connection)?.ok_or_else(|| {
        DeviceError::new("identity_unavailable", "local device is not initialized")
    })?;
    let mut statement = connection
        .prepare("SELECT record_json FROM trust_records ORDER BY record_id")
        .map_err(db_error)?;
    let rows = statement
        .query_map([], |row| row.get::<_, Vec<u8>>(0))
        .map_err(db_error)?;
    let mut devices = BTreeMap::new();
    let mut relationships = Vec::new();
    let mut profile_grants = Vec::new();
    let mut grants = Vec::new();
    let mut revocations = Vec::new();
    let mut names = Vec::new();
    let mut builds = Vec::new();
    let mut lifecycles = BTreeMap::new();
    for row in rows {
        let record: TrustRecord =
            serde_json::from_slice(&row.map_err(db_error)?).map_err(json_error)?;
        match record {
            TrustRecord::EndpointBinding(_) | TrustRecord::Unknown => {}
            TrustRecord::ProfileGrant(value) => profile_grants.push(value),
            TrustRecord::ProfileLifecycle(value) => {
                match koru_device::compare_profile_lifecycle(
                    lifecycles.get(&value.profile_id),
                    &value,
                ) {
                    koru_device::ProfileLifecycleOrder::Apply => {
                        lifecycles.insert(value.profile_id, value);
                    }
                    koru_device::ProfileLifecycleOrder::Duplicate
                    | koru_device::ProfileLifecycleOrder::Stale => {}
                    koru_device::ProfileLifecycleOrder::Conflict => {
                        return Err(DeviceError::new(
                            "profile_lifecycle_conflict",
                            "stored equal Profile lifecycle revisions disagree",
                        ));
                    }
                }
            }
            TrustRecord::Relationship(value) => {
                devices.insert(value.inviter.device_id, value.inviter.clone());
                devices.insert(value.joiner.device_id, value.joiner.clone());
                relationships.push(value);
            }
            TrustRecord::AllDataGrant(value) => grants.push(value),
            TrustRecord::Revocation(value) => revocations.push(value),
            TrustRecord::DeviceName(value) => names.push(value),
            TrustRecord::DeviceBuild(value) => builds.push(value),
        }
    }
    devices.remove(&local_device.device_id);
    Ok(TrustView {
        local_device,
        devices: devices.into_values().collect(),
        relationships,
        profile_grants,
        grants,
        revocations,
        names,
        builds,
        lifecycles,
        effective_revision: connection
            .query_row(
                "SELECT effective_revision FROM trust_meta WHERE singleton = 1",
                [],
                |row| row.get(0),
            )
            .map_err(db_error)?,
    })
}

fn current_profile_lifecycle(
    connection: &Connection,
    profile: koru_store_core::ProfileId,
) -> Result<Option<koru_device::ProfileLifecycleV1>, DeviceError> {
    let mut statement = connection
        .prepare("SELECT record_json FROM trust_records WHERE kind = 'profile_lifecycle'")
        .map_err(db_error)?;
    let rows = statement
        .query_map([], |row| row.get::<_, Vec<u8>>(0))
        .map_err(db_error)?;
    let mut current = None;
    for row in rows {
        let record: TrustRecord =
            serde_json::from_slice(&row.map_err(db_error)?).map_err(json_error)?;
        let TrustRecord::ProfileLifecycle(candidate) = record else {
            continue;
        };
        if candidate.profile_id != profile {
            continue;
        }
        match koru_device::compare_profile_lifecycle(current.as_ref(), &candidate) {
            koru_device::ProfileLifecycleOrder::Apply => current = Some(candidate),
            koru_device::ProfileLifecycleOrder::Duplicate
            | koru_device::ProfileLifecycleOrder::Stale => {}
            koru_device::ProfileLifecycleOrder::Conflict => {
                return Err(DeviceError::new(
                    "profile_lifecycle_conflict",
                    "stored equal Profile lifecycle revisions disagree",
                ));
            }
        }
    }
    Ok(current)
}

fn prune_superseded_lifecycles(
    transaction: &rusqlite::Transaction<'_>,
    written: &koru_device::ProfileLifecycleV1,
) -> Result<(), DeviceError> {
    let mut statement = transaction
        .prepare(
            "SELECT record_id, record_json FROM trust_records WHERE kind = 'profile_lifecycle'",
        )
        .map_err(db_error)?;
    let rows = statement
        .query_map([], |row| {
            Ok((row.get::<_, String>(0)?, row.get::<_, Vec<u8>>(1)?))
        })
        .map_err(db_error)?;
    let mut superseded = Vec::new();
    for row in rows {
        let (record_id, bytes) = row.map_err(db_error)?;
        let Ok(TrustRecord::ProfileLifecycle(candidate)) =
            serde_json::from_slice::<TrustRecord>(&bytes)
        else {
            continue;
        };
        if candidate.profile_id == written.profile_id && candidate.record_id != written.record_id {
            superseded.push(record_id);
        }
    }
    drop(statement);
    for record_id in superseded {
        transaction
            .execute(
                "DELETE FROM trust_records WHERE record_id = ?1",
                params![record_id],
            )
            .map_err(db_error)?;
    }
    Ok(())
}

/// Keep one build description per device: the latest by the device's own
/// clock, the record id as tie-break — the order `TrustView::build_of`
/// resolves by — whether the row just written is that one or a straggler
/// that arrived after it.
fn prune_superseded_builds(
    transaction: &rusqlite::Transaction<'_>,
    written: &koru_device::DeviceBuildV1,
) -> Result<(), DeviceError> {
    let mut statement = transaction
        .prepare("SELECT record_id, record_json FROM trust_records WHERE kind = 'device_build'")
        .map_err(db_error)?;
    let rows = statement
        .query_map([], |row| {
            Ok((row.get::<_, String>(0)?, row.get::<_, Vec<u8>>(1)?))
        })
        .map_err(db_error)?;
    let mut same_device = Vec::new();
    for row in rows {
        let (record_id, bytes) = row.map_err(db_error)?;
        let Ok(TrustRecord::DeviceBuild(build)) = serde_json::from_slice::<TrustRecord>(&bytes)
        else {
            continue;
        };
        if build.device_id == written.device_id {
            same_device.push(((build.issued_at_millis, build.record_id), record_id));
        }
    }
    drop(statement);
    let Some(latest) = same_device.iter().map(|(order, _)| *order).max() else {
        return Ok(());
    };
    let superseded = same_device
        .into_iter()
        .filter(|(order, _)| *order != latest)
        .map(|(_, record_id)| record_id);
    for record_id in superseded {
        transaction
            .execute(
                "DELETE FROM trust_records WHERE record_id = ?1",
                params![record_id],
            )
            .map_err(db_error)?;
    }
    Ok(())
}

fn db_error(error: rusqlite::Error) -> DeviceError {
    DeviceError::new("identity_database", error.to_string())
}
fn json_error(error: serde_json::Error) -> DeviceError {
    DeviceError::new("identity_corrupt", error.to_string())
}
fn io_error(error: std::io::Error) -> DeviceError {
    DeviceError::new("identity_io", error.to_string())
}

#[cfg(test)]
mod tests;

#[cfg(test)]
mod migration_tests;

fn read_local_device(connection: &Connection) -> Result<Option<DeviceDescriptorV1>, DeviceError> {
    let bytes: Option<Vec<u8>> = connection
        .query_row(
            "SELECT descriptor_json FROM local_device WHERE singleton = 1",
            [],
            |row| row.get(0),
        )
        .optional()
        .map_err(db_error)?;
    bytes
        .map(|bytes| serde_json::from_slice(&bytes).map_err(json_error))
        .transpose()
}

fn legacy_error() -> DeviceError {
    DeviceError::new(
        "trust_migration_failed",
        "legacy trust could not be imported intact",
    )
}

fn record_kind(record: &TrustRecord) -> Result<&'static str, DeviceError> {
    Ok(match record {
        TrustRecord::EndpointBinding(_) => "endpoint_binding",
        TrustRecord::ProfileGrant(_) => "profile_grant",
        TrustRecord::ProfileLifecycle(_) => "profile_lifecycle",
        TrustRecord::Relationship(_) => "relationship",
        TrustRecord::AllDataGrant(_) => "all_data_grant",
        TrustRecord::Revocation(_) => "revocation",
        TrustRecord::DeviceName(_) => "device_name",
        TrustRecord::DeviceBuild(_) => "device_build",
        TrustRecord::Unknown => {
            return Err(DeviceError::new(
                "unknown_record_kind",
                "a record kind this build does not know is never stored",
            ));
        }
    })
}
