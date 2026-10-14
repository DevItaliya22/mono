// SPDX-License-Identifier: MPL-2.0

use super::*;
use koru_account_core::{AuthIdentityId, KoruIdentityId};
use koru_store_core::ProfileId;
use std::collections::BTreeSet;

#[derive(Clone, Copy, PartialEq, Eq)]
pub struct AccountOwner {
    pub account: KoruIdentityId,
    pub auth_identity: AuthIdentityId,
}

impl std::fmt::Debug for AccountOwner {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str("AccountOwner(REDACTED)")
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum DirectoryState {
    Pending,
    Ready,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct DirectoryRecord {
    pub owner: AccountOwner,
    pub state: DirectoryState,
    /// Namespace-monotonic CAS revision, never reused after removal.
    pub version: i64,
}

#[derive(Clone, Default)]
pub struct AccountDirectorySnapshot {
    pub records: Vec<DirectoryRecord>,
    pub active: Option<KoruIdentityId>,
    pub legacy_retired: bool,
}

fn invalid() -> DeviceError {
    DeviceError::new(
        "account_directory_invalid",
        "invalid account directory state",
    )
}
fn conflict() -> DeviceError {
    DeviceError::new("account_directory_changed", "account directory changed")
}
fn validate_namespace(namespace: &str) -> Result<(), DeviceError> {
    if namespace.len() == 64
        && namespace
            .bytes()
            .all(|b| b.is_ascii_digit() || (b'a'..=b'f').contains(&b))
    {
        Ok(())
    } else {
        Err(invalid())
    }
}

impl SqliteTrustRepository {
    pub fn account_directory(
        &self,
        namespace: &str,
    ) -> Result<AccountDirectorySnapshot, DeviceError> {
        validate_namespace(namespace)?;
        let transaction = self.connection.unchecked_transaction().map_err(db_error)?;
        let metadata: Option<(Option<String>, bool)> = transaction.query_row(
            "SELECT active_account_id, legacy_retired FROM account_namespace WHERE namespace = ?1",
            [namespace], |row| Ok((row.get(0)?, row.get(1)?)),
        ).optional().map_err(db_error)?;
        let mut snapshot = AccountDirectorySnapshot::default();
        if let Some((active, retired)) = metadata {
            snapshot.active = active
                .map(|value| value.parse().map_err(|_| invalid()))
                .transpose()?;
            snapshot.legacy_retired = retired;
        }
        {
            let mut statement = transaction.prepare(
                "SELECT account_id, auth_identity_id, state, record_version FROM account_directory WHERE namespace = ?1 ORDER BY account_id"
            ).map_err(db_error)?;
            let rows = statement
                .query_map([namespace], |row| {
                    Ok((
                        row.get::<_, String>(0)?,
                        row.get::<_, String>(1)?,
                        row.get::<_, String>(2)?,
                        row.get::<_, i64>(3)?,
                    ))
                })
                .map_err(db_error)?;
            for row in rows {
                let (account, auth, state, version) = row.map_err(db_error)?;
                snapshot.records.push(DirectoryRecord {
                    owner: AccountOwner {
                        account: account.parse().map_err(|_| invalid())?,
                        auth_identity: auth.parse().map_err(|_| invalid())?,
                    },
                    state: match state.as_str() {
                        "pending" => DirectoryState::Pending,
                        "ready" => DirectoryState::Ready,
                        _ => return Err(invalid()),
                    },
                    version,
                });
            }
        }
        transaction.commit().map_err(db_error)?;
        Ok(snapshot)
    }

    /// Commit discovery before touching the vault. Existing targets stay with
    /// their account even when another Auth Identity is replacing its session.
    pub fn begin_account_write(
        &mut self,
        namespace: &str,
        owner: AccountOwner,
    ) -> Result<DirectoryRecord, DeviceError> {
        validate_namespace(namespace)?;
        let transaction = self
            .connection
            .transaction_with_behavior(rusqlite::TransactionBehavior::Immediate)
            .map_err(db_error)?;
        transaction.execute("INSERT INTO account_namespace(namespace) VALUES(?1) ON CONFLICT(namespace) DO NOTHING", [namespace]).map_err(db_error)?;
        let previous: i64 = transaction
            .query_row(
                "SELECT next_record_version FROM account_namespace WHERE namespace = ?1",
                [namespace],
                |row| row.get(0),
            )
            .map_err(db_error)?;
        let version = previous.checked_add(1).ok_or_else(invalid)?;
        transaction
            .execute(
                "UPDATE account_namespace SET next_record_version = ?2 WHERE namespace = ?1",
                params![namespace, version],
            )
            .map_err(db_error)?;
        transaction.execute("INSERT INTO account_directory(namespace, account_id, auth_identity_id, state, record_version) VALUES(?1, ?2, ?3, 'pending', ?4)
            ON CONFLICT(namespace, account_id) DO UPDATE SET auth_identity_id = excluded.auth_identity_id, state = 'pending', record_version = excluded.record_version",
            params![namespace, owner.account.to_string(), owner.auth_identity.to_string(), version]).map_err(db_error)?;
        transaction.execute("UPDATE account_namespace SET active_account_id = ?2 WHERE namespace = ?1 AND active_account_id IS NULL", params![namespace, owner.account.to_string()]).map_err(db_error)?;
        transaction.commit().map_err(db_error)?;
        Ok(DirectoryRecord {
            owner,
            state: DirectoryState::Pending,
            version,
        })
    }

    /// A late ACK is a conditional update, never an insertion. Retirement is
    /// committed with discoverable ready state and survives removing the account.
    pub fn finish_account_write(
        &mut self,
        namespace: &str,
        record: &DirectoryRecord,
        actual: AccountOwner,
        activate: bool,
        retire_legacy: bool,
    ) -> Result<(), DeviceError> {
        validate_namespace(namespace)?;
        if actual.account != record.owner.account {
            return Err(conflict());
        }
        let transaction = self
            .connection
            .transaction_with_behavior(rusqlite::TransactionBehavior::Immediate)
            .map_err(db_error)?;
        let changed = transaction.execute("UPDATE account_directory SET state = 'ready', auth_identity_id = ?5 WHERE namespace = ?1 AND account_id = ?2 AND record_version = ?3 AND (auth_identity_id = ?4 OR (state = 'ready' AND auth_identity_id = ?5))",
            params![namespace, record.owner.account.to_string(), record.version, record.owner.auth_identity.to_string(), actual.auth_identity.to_string()]).map_err(db_error)?;
        if changed != 1 {
            return Err(conflict());
        }
        if activate {
            transaction
                .execute(
                    "UPDATE account_namespace SET active_account_id = ?2 WHERE namespace = ?1",
                    params![namespace, actual.account.to_string()],
                )
                .map_err(db_error)?;
        }
        if retire_legacy {
            transaction
                .execute(
                    "UPDATE account_namespace SET legacy_retired = 1 WHERE namespace = ?1",
                    [namespace],
                )
                .map_err(db_error)?;
        }
        transaction.commit().map_err(db_error)
    }

    pub fn remove_account(
        &mut self,
        namespace: &str,
        record: &DirectoryRecord,
    ) -> Result<(), DeviceError> {
        validate_namespace(namespace)?;
        let transaction = self
            .connection
            .transaction_with_behavior(rusqlite::TransactionBehavior::Immediate)
            .map_err(db_error)?;
        let version: Option<i64> = transaction.query_row("SELECT record_version FROM account_directory WHERE namespace = ?1 AND account_id = ?2", params![namespace, record.owner.account.to_string()], |row| row.get(0)).optional().map_err(db_error)?;
        if version.is_some_and(|version| version != record.version) {
            return Err(conflict());
        }
        transaction.execute("UPDATE account_namespace SET active_account_id = NULL WHERE namespace = ?1 AND active_account_id = ?2", params![namespace, record.owner.account.to_string()]).map_err(db_error)?;
        transaction.execute("DELETE FROM account_directory WHERE namespace = ?1 AND account_id = ?2 AND record_version = ?3", params![namespace, record.owner.account.to_string(), record.version]).map_err(db_error)?;
        transaction.commit().map_err(db_error)
    }

    pub fn activate_account(
        &mut self,
        namespace: &str,
        account: KoruIdentityId,
    ) -> Result<(), DeviceError> {
        validate_namespace(namespace)?;
        // Selection is discovery metadata, including an owned replacement
        // awaiting vault acknowledgement. Authority still requires ready/version.
        let changed = self.connection.execute("UPDATE account_namespace SET active_account_id = ?2 WHERE namespace = ?1 AND EXISTS(SELECT 1 FROM account_directory WHERE namespace = ?1 AND account_id = ?2)", params![namespace, account.to_string()]).map_err(db_error)?;
        if changed == 1 {
            Ok(())
        } else {
            Err(conflict())
        }
    }

    pub fn retire_legacy_credential(&mut self, namespace: &str) -> Result<(), DeviceError> {
        validate_namespace(namespace)?;
        self.connection.execute("INSERT INTO account_namespace(namespace, legacy_retired) VALUES(?1, 1) ON CONFLICT(namespace) DO UPDATE SET legacy_retired = 1", [namespace]).map_err(db_error)?;
        Ok(())
    }

    pub fn deactivate_account(
        &mut self,
        namespace: &str,
        account: KoruIdentityId,
    ) -> Result<(), DeviceError> {
        validate_namespace(namespace)?;
        self.connection.execute("UPDATE account_namespace SET active_account_id = NULL WHERE namespace = ?1 AND active_account_id = ?2", params![namespace, account.to_string()]).map_err(db_error)?;
        Ok(())
    }

    pub fn account_targets(
        &self,
        namespace: &str,
        account: KoruIdentityId,
    ) -> Result<BTreeSet<ProfileId>, DeviceError> {
        validate_namespace(namespace)?;
        read_targets(&self.connection, namespace, account)
    }

    /// Validate an acknowledged refresh's current revision and unchanged set.
    /// Unlike empty-set cleanup, a missing account is always a refusal.
    pub fn validate_account_targets(
        &mut self,
        namespace: &str,
        owner: AccountOwner,
        version: i64,
        expected: &BTreeSet<ProfileId>,
    ) -> Result<(), DeviceError> {
        validate_namespace(namespace)?;
        let transaction = self
            .connection
            .transaction_with_behavior(rusqlite::TransactionBehavior::Immediate)
            .map_err(db_error)?;
        require_ready_owner(&transaction, namespace, owner, version)?;
        if read_targets(&transaction, namespace, owner.account)? != *expected {
            return Err(conflict());
        }
        transaction.commit().map_err(db_error)
    }

    /// Replace only this account's exact unfinished set. Expected-set equality
    /// fences late completion and prevents it from erasing a newer selection.
    pub fn replace_account_targets(
        &mut self,
        namespace: &str,
        owner: AccountOwner,
        version: i64,
        expected: &BTreeSet<ProfileId>,
        next: &BTreeSet<ProfileId>,
    ) -> Result<(), DeviceError> {
        validate_namespace(namespace)?;
        let transaction = self
            .connection
            .transaction_with_behavior(rusqlite::TransactionBehavior::Immediate)
            .map_err(db_error)?;
        if next.is_empty() {
            let exists: bool = transaction.query_row("SELECT EXISTS(SELECT 1 FROM account_directory WHERE namespace = ?1 AND account_id = ?2)", params![namespace, owner.account.to_string()], |row| row.get(0)).map_err(db_error)?;
            if !exists {
                return transaction.commit().map_err(db_error);
            }
        }
        require_ready_owner(&transaction, namespace, owner, version)?;
        if read_targets(&transaction, namespace, owner.account)? != *expected {
            return Err(conflict());
        }
        transaction
            .execute(
                "DELETE FROM enrollment_targets WHERE namespace = ?1 AND account_id = ?2",
                params![namespace, owner.account.to_string()],
            )
            .map_err(db_error)?;
        for profile in next {
            transaction.execute("INSERT INTO enrollment_targets(namespace, account_id, profile_id) VALUES(?1, ?2, ?3)", params![namespace, owner.account.to_string(), profile.to_storage()]).map_err(db_error)?;
        }
        transaction.commit().map_err(db_error)
    }

    /// Caller supplies authenticated Profile owner/exact-Auth authorization and
    /// has checked canonical local binding. This storage port never invents proof.
    pub fn claim_unattributed_target(
        &mut self,
        namespace: &str,
        owner: AccountOwner,
        version: i64,
        profile: ProfileId,
    ) -> Result<bool, DeviceError> {
        validate_namespace(namespace)?;
        let transaction = self
            .connection
            .transaction_with_behavior(rusqlite::TransactionBehavior::Immediate)
            .map_err(db_error)?;
        require_ready_owner(&transaction, namespace, owner, version)?;
        let removed = transaction
            .execute(
                "DELETE FROM enrollment_unattributed WHERE singleton = 1 AND profile_id = ?1",
                [profile.to_storage()],
            )
            .map_err(db_error)?;
        if removed == 1 {
            transaction.execute("INSERT INTO enrollment_targets(namespace, account_id, profile_id) VALUES(?1, ?2, ?3) ON CONFLICT DO NOTHING", params![namespace, owner.account.to_string(), profile.to_storage()]).map_err(db_error)?;
        }
        transaction.commit().map_err(db_error)?;
        Ok(removed == 1)
    }
}

fn require_ready_owner(
    connection: &Connection,
    namespace: &str,
    owner: AccountOwner,
    version: i64,
) -> Result<(), DeviceError> {
    let found: bool = connection.query_row("SELECT EXISTS(SELECT 1 FROM account_directory WHERE namespace = ?1 AND account_id = ?2 AND auth_identity_id = ?3 AND record_version = ?4 AND state = 'ready')", params![namespace, owner.account.to_string(), owner.auth_identity.to_string(), version], |row| row.get(0)).map_err(db_error)?;
    if found { Ok(()) } else { Err(conflict()) }
}
fn read_targets(
    connection: &Connection,
    namespace: &str,
    account: KoruIdentityId,
) -> Result<BTreeSet<ProfileId>, DeviceError> {
    let mut statement = connection.prepare("SELECT profile_id FROM enrollment_targets WHERE namespace = ?1 AND account_id = ?2 ORDER BY profile_id").map_err(db_error)?;
    let rows = statement
        .query_map(params![namespace, account.to_string()], |row| {
            row.get::<_, String>(0)
        })
        .map_err(db_error)?;
    rows.map(|row| ProfileId::parse(&row.map_err(db_error)?).ok_or_else(invalid))
        .collect()
}

#[cfg(test)]
mod tests;
