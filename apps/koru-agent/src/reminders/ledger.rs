// SPDX-License-Identifier: MPL-2.0

use std::fs::{self, OpenOptions};
use std::io::Write;
use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};

use super::{FailureCategory, Ledger, LedgerState, SchedulerFailure};

const SCHEMA_VERSION: u8 = 1;

#[derive(Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct LedgerFile {
    schema_version: u8,
    state: LedgerState,
}

enum DecodeError {
    Malformed,
    FutureSchema,
}

pub(crate) trait LedgerIo: Send {
    fn read(&mut self, path: &Path) -> std::io::Result<Option<Vec<u8>>>;
    fn write_synced(&mut self, path: &Path, bytes: &[u8]) -> std::io::Result<()>;
    fn rename(&mut self, from: &Path, to: &Path) -> std::io::Result<()>;
    fn remove(&mut self, path: &Path) -> std::io::Result<()>;
    fn create_parent(&mut self, path: &Path) -> std::io::Result<()>;
}

pub(crate) struct StdLedgerIo;

impl LedgerIo for StdLedgerIo {
    fn read(&mut self, path: &Path) -> std::io::Result<Option<Vec<u8>>> {
        match fs::read(path) {
            Ok(bytes) => Ok(Some(bytes)),
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(None),
            Err(error) => Err(error),
        }
    }

    fn write_synced(&mut self, path: &Path, bytes: &[u8]) -> std::io::Result<()> {
        let mut file = OpenOptions::new()
            .create(true)
            .truncate(true)
            .write(true)
            .open(path)?;
        file.write_all(bytes)?;
        file.sync_all()
    }

    fn rename(&mut self, from: &Path, to: &Path) -> std::io::Result<()> {
        fs::rename(from, to)
    }

    fn remove(&mut self, path: &Path) -> std::io::Result<()> {
        match fs::remove_file(path) {
            Ok(()) => Ok(()),
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(()),
            Err(error) => Err(error),
        }
    }

    fn create_parent(&mut self, path: &Path) -> std::io::Result<()> {
        if let Some(parent) = path.parent() {
            fs::create_dir_all(parent)?;
        }
        Ok(())
    }
}

/// Atomic, recoverable storage for rebuildable native registration facts.
pub(crate) struct FileLedger<I = StdLedgerIo> {
    path: PathBuf,
    io: I,
    native_reset_required: bool,
}

impl FileLedger<StdLedgerIo> {
    pub(crate) fn new(path: PathBuf) -> Self {
        Self {
            path,
            io: StdLedgerIo,
            native_reset_required: false,
        }
    }
}

impl<I: LedgerIo> FileLedger<I> {
    fn sibling(&self, extension: &str) -> PathBuf {
        let mut name = self.path.as_os_str().to_owned();
        name.push(extension);
        PathBuf::from(name)
    }

    fn decode(bytes: &[u8]) -> Result<LedgerState, DecodeError> {
        let value: serde_json::Value =
            serde_json::from_slice(bytes).map_err(|_| DecodeError::Malformed)?;
        let schema = value
            .get("schema_version")
            .and_then(serde_json::Value::as_u64)
            .ok_or(DecodeError::Malformed)?;
        if schema > u64::from(SCHEMA_VERSION) {
            return Err(DecodeError::FutureSchema);
        }
        if schema != u64::from(SCHEMA_VERSION) {
            return Err(DecodeError::Malformed);
        }
        let file: LedgerFile = serde_json::from_value(value).map_err(|_| DecodeError::Malformed)?;
        validate(&file.state)?;
        Ok(file.state)
    }

    fn failure() -> SchedulerFailure {
        SchedulerFailure::new(FailureCategory::Ledger)
    }
}

impl<I: LedgerIo> Ledger for FileLedger<I> {
    fn load(&mut self) -> Result<LedgerState, SchedulerFailure> {
        self.native_reset_required = false;
        let previous = self.sibling(".previous");
        let primary = self.io.read(&self.path).map_err(|_| Self::failure())?;
        if let Some(bytes) = primary.as_deref() {
            match Self::decode(bytes) {
                Ok(state) => return Ok(state),
                Err(DecodeError::FutureSchema) => return Err(Self::failure()),
                Err(DecodeError::Malformed) => {}
            }
        }
        let backup = self.io.read(&previous).map_err(|_| Self::failure())?;
        if let Some(bytes) = backup.as_deref() {
            match Self::decode(bytes) {
                Ok(state) => {
                    self.native_reset_required = true;
                    return Ok(state);
                }
                Err(DecodeError::FutureSchema) => return Err(Self::failure()),
                Err(DecodeError::Malformed) => {}
            }
        }
        // Store can recreate desired registrations, but it cannot name an
        // obsolete OS registration after this cache is lost. Signal the
        // scheduler to clear only this root's native namespace first.
        self.native_reset_required = true;
        Ok(LedgerState::default())
    }

    fn replace(&mut self, state: &LedgerState) -> Result<(), SchedulerFailure> {
        let part = self.sibling(".part");
        let previous = self.sibling(".previous");
        let bytes = serde_json::to_vec(&LedgerFile {
            schema_version: SCHEMA_VERSION,
            state: state.clone(),
        })
        .map_err(|_| Self::failure())?;

        self.io
            .create_parent(&self.path)
            .and_then(|()| self.io.write_synced(&part, &bytes))
            .map_err(|_| Self::failure())?;
        let staged = self.io.read(&part).map_err(|_| Self::failure())?;
        if staged
            .as_deref()
            .map(Self::decode)
            .transpose()
            .ok()
            .flatten()
            .as_ref()
            != Some(state)
        {
            return Err(Self::failure());
        }

        self.io.remove(&previous).map_err(|_| Self::failure())?;
        let had_primary = self
            .io
            .read(&self.path)
            .map_err(|_| Self::failure())?
            .is_some();
        if had_primary {
            self.io
                .rename(&self.path, &previous)
                .map_err(|_| Self::failure())?;
        }
        if self.io.rename(&part, &self.path).is_err() {
            if had_primary {
                let _ = self.io.rename(&previous, &self.path);
            }
            return Err(Self::failure());
        }
        self.native_reset_required = false;
        Ok(())
    }

    fn native_reset_required(&self) -> bool {
        self.native_reset_required
    }
}

fn validate(state: &LedgerState) -> Result<(), DecodeError> {
    let mut registrations = std::collections::HashSet::new();
    for entry in &state.entries {
        if !registrations.insert(entry.registration.as_str()) {
            return Err(DecodeError::Malformed);
        }
        if entry.native_handle.as_ref().is_some_and(invalid_handle) {
            return Err(DecodeError::Malformed);
        }
        if let Some(rules) = &entry.fingerprint.floating_rules
            && (rules.canonical_zone.is_empty()
                || rules.canonical_zone.len() > 128
                || rules.canonical_zone.chars().any(char::is_control)
                || rules.database_version.is_empty()
                || rules.database_version.len() > 64
                || rules.database_version.chars().any(char::is_control)
                || koru_time_zone::IanaZoneRules::parse(&rules.canonical_zone).is_err()
                || koru_time_zone::IanaZoneRules::parse(&rules.canonical_zone)
                    .is_ok_and(|zone| zone.canonical_name().as_str() != rules.canonical_zone))
        {
            return Err(DecodeError::Malformed);
        }
    }
    let mut profiles = std::collections::HashSet::new();
    for summary in &state.quiet_summaries {
        if !profiles.insert(summary.profile)
            || !registrations.insert(summary.registration.as_str())
            || summary.count == 0
            || invalid_handle(&summary.native_handle)
        {
            return Err(DecodeError::Malformed);
        }
    }
    Ok(())
}

fn invalid_handle(handle: &koru_native_notifications::NativeHandle) -> bool {
    handle.0.is_empty() || handle.0.len() > 256 || handle.0.chars().any(char::is_control)
}

#[cfg(test)]
mod tests {
    use std::collections::HashMap;

    use koru_native_notifications::{NativeHandle, RegistrationId};
    use koru_store_core::ids::{ProfileId, ReminderId, RuleRevisionId};
    use koru_store_core::reminders::{OccurrenceKey, ToneId};

    use super::*;
    use crate::reminders::{LedgerEntry, RegistrationStatus, ScheduleFingerprint};

    #[derive(Default)]
    struct FakeIo {
        files: HashMap<PathBuf, Vec<u8>>,
        fail_final_rename: bool,
    }

    impl LedgerIo for FakeIo {
        fn read(&mut self, path: &Path) -> std::io::Result<Option<Vec<u8>>> {
            Ok(self.files.get(path).cloned())
        }

        fn write_synced(&mut self, path: &Path, bytes: &[u8]) -> std::io::Result<()> {
            self.files.insert(path.to_owned(), bytes.to_vec());
            Ok(())
        }

        fn rename(&mut self, from: &Path, to: &Path) -> std::io::Result<()> {
            if self.fail_final_rename && from.as_os_str().to_string_lossy().ends_with(".part") {
                self.fail_final_rename = false;
                return Err(std::io::Error::other("armed rename failure"));
            }
            let bytes = self
                .files
                .remove(from)
                .ok_or_else(|| std::io::Error::from(std::io::ErrorKind::NotFound))?;
            self.files.insert(to.to_owned(), bytes);
            Ok(())
        }

        fn remove(&mut self, path: &Path) -> std::io::Result<()> {
            self.files.remove(path);
            Ok(())
        }

        fn create_parent(&mut self, _: &Path) -> std::io::Result<()> {
            Ok(())
        }
    }

    fn state(cursor: u64) -> LedgerState {
        let reminder = ReminderId::new();
        let revision = RuleRevisionId::new();
        LedgerState {
            event_cursor: Some(cursor),
            entries: vec![LedgerEntry {
                profile: ProfileId::new(),
                reminder,
                revision,
                occurrence: OccurrenceKey::parse(&format!("occ1-{}", "01".repeat(32))).unwrap(),
                registration: RegistrationId::new("registration-1").unwrap(),
                native_handle: Some(NativeHandle("native-1".into())),
                fingerprint: ScheduleFingerprint {
                    deliver_at_ms: 42,
                    tone: ToneId::Soft,
                    floating_rules: None,
                },
                status: RegistrationStatus::Registered,
                fired_at_ms: None,
            }],
            quiet_summaries: Vec::new(),
        }
    }

    #[test]
    fn replacement_round_trips_without_authored_content() {
        let path = PathBuf::from("selected/agent/reminder-delivery-v1.json");
        let mut ledger = FileLedger {
            path: path.clone(),
            io: FakeIo::default(),
            native_reset_required: false,
        };
        let expected = state(7);
        ledger.replace(&expected).unwrap();
        assert_eq!(ledger.load().unwrap(), expected);
        let json = String::from_utf8(ledger.io.files[&path].clone()).unwrap();
        for forbidden in ["title", "body", "workspace", "document", "node", "entity"] {
            assert!(
                !json.contains(forbidden),
                "ledger leaked {forbidden}: {json}"
            );
        }
    }

    #[test]
    fn corrupt_primary_recovers_previous_commit() {
        let path = PathBuf::from("agent/ledger.json");
        let mut ledger = FileLedger {
            path: path.clone(),
            io: FakeIo::default(),
            native_reset_required: false,
        };
        let first = state(1);
        let second = state(2);
        ledger.replace(&first).unwrap();
        ledger.replace(&second).unwrap();
        ledger.io.files.insert(path, b"not json".to_vec());
        assert_eq!(ledger.load().unwrap(), first);
        assert!(ledger.native_reset_required());
    }

    #[test]
    fn failed_publish_restores_the_last_primary() {
        let path = PathBuf::from("agent/ledger.json");
        let mut ledger = FileLedger {
            path: path.clone(),
            io: FakeIo::default(),
            native_reset_required: false,
        };
        let first = state(1);
        ledger.replace(&first).unwrap();
        ledger.io.fail_final_rename = true;
        assert!(ledger.replace(&state(2)).is_err());
        assert_eq!(ledger.load().unwrap(), first);
    }

    #[test]
    fn malformed_primary_and_backup_rebuild_empty() {
        let path = PathBuf::from("agent/ledger.json");
        let previous = PathBuf::from("agent/ledger.json.previous");
        let mut io = FakeIo::default();
        io.files.insert(path.clone(), b"broken".to_vec());
        io.files
            .insert(previous, br#"{"schema_version":1,"state":{}}"#.to_vec());
        let mut ledger = FileLedger {
            path,
            io,
            native_reset_required: false,
        };
        assert_eq!(ledger.load().unwrap(), LedgerState::default());
        assert!(ledger.native_reset_required());
    }

    #[test]
    fn future_schema_is_never_treated_as_rebuildable_corruption() {
        let path = PathBuf::from("agent/ledger.json");
        let mut io = FakeIo::default();
        io.files.insert(
            path.clone(),
            br#"{"schema_version":2,"state":{"event_cursor":null,"entries":[]}}"#.to_vec(),
        );
        let mut ledger = FileLedger {
            path,
            io,
            native_reset_required: false,
        };
        assert_eq!(ledger.load().unwrap_err().category, FailureCategory::Ledger);
    }

    #[test]
    fn schema_one_without_summary_registrations_remains_readable() {
        let Ok(decoded) = FileLedger::<FakeIo>::decode(
            br#"{"schema_version":1,"state":{"event_cursor":null,"entries":[]}}"#,
        ) else {
            panic!("the previous schema-one shape must decode")
        };
        assert!(decoded.quiet_summaries.is_empty());
    }

    #[test]
    fn semantic_validation_rejects_duplicate_registration_ids() {
        let path = PathBuf::from("agent/ledger.json");
        let mut invalid = state(1);
        invalid.entries.push(invalid.entries[0].clone());
        let bytes = serde_json::to_vec(&LedgerFile {
            schema_version: SCHEMA_VERSION,
            state: invalid,
        })
        .unwrap();
        let mut io = FakeIo::default();
        io.files.insert(path.clone(), bytes);
        let mut ledger = FileLedger {
            path,
            io,
            native_reset_required: false,
        };
        assert_eq!(ledger.load().unwrap(), LedgerState::default());
    }

    #[test]
    fn semantic_validation_rejects_noncanonical_or_control_character_zones() {
        for zone in ["US/Eastern", "America/New_York\n"] {
            let path = PathBuf::from("agent/ledger.json");
            let mut invalid = state(1);
            invalid.entries[0].fingerprint.floating_rules =
                Some(super::super::ZoneRulesFingerprint {
                    canonical_zone: zone.into(),
                    database_version: "2025b".into(),
                });
            let bytes = serde_json::to_vec(&LedgerFile {
                schema_version: SCHEMA_VERSION,
                state: invalid,
            })
            .unwrap();
            let mut io = FakeIo::default();
            io.files.insert(path.clone(), bytes);
            let mut ledger = FileLedger {
                path,
                io,
                native_reset_required: false,
            };
            assert_eq!(ledger.load().unwrap(), LedgerState::default());
        }
    }
}
