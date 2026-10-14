// SPDX-License-Identifier: MPL-2.0

use std::fs::{self, OpenOptions};
use std::io::Write;
use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};

use super::{FailureCategory, SchedulerFailure};

const SCHEMA_VERSION: u8 = 1;

/// The Agent-owned, machine-local delivery choice.
///
/// This deliberately does not share the rebuildable registration ledger: a
/// corrupt or deleted cache must not reverse a person's choice. It also does
/// not enter profile state, so one device can never control another.
pub(crate) trait DeliveryPreferences: Send {
    fn load_enabled(&mut self) -> Result<bool, SchedulerFailure>;
    fn set_enabled(&mut self, enabled: bool) -> Result<(), SchedulerFailure>;
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) struct MemoryDeliveryPreferences {
    enabled: bool,
}

impl Default for MemoryDeliveryPreferences {
    fn default() -> Self {
        Self { enabled: true }
    }
}

impl DeliveryPreferences for MemoryDeliveryPreferences {
    fn load_enabled(&mut self) -> Result<bool, SchedulerFailure> {
        Ok(self.enabled)
    }

    fn set_enabled(&mut self, enabled: bool) -> Result<(), SchedulerFailure> {
        self.enabled = enabled;
        Ok(())
    }
}

#[derive(Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct PreferenceFile {
    schema_version: u8,
    enabled: bool,
}

enum DecodeError {
    Malformed,
    FutureSchema,
}

pub(crate) trait PreferenceIo: Send {
    fn read(&mut self, path: &Path) -> std::io::Result<Option<Vec<u8>>>;
    fn write_synced(&mut self, path: &Path, bytes: &[u8]) -> std::io::Result<()>;
    fn rename(&mut self, from: &Path, to: &Path) -> std::io::Result<()>;
    fn remove(&mut self, path: &Path) -> std::io::Result<()>;
    fn create_parent(&mut self, path: &Path) -> std::io::Result<()>;
}

pub(crate) struct StdPreferenceIo;

impl PreferenceIo for StdPreferenceIo {
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

pub(crate) struct FileDeliveryPreferences<I = StdPreferenceIo> {
    path: PathBuf,
    io: I,
}

impl FileDeliveryPreferences<StdPreferenceIo> {
    pub(crate) fn new(path: PathBuf) -> Self {
        Self {
            path,
            io: StdPreferenceIo,
        }
    }
}

impl<I: PreferenceIo> FileDeliveryPreferences<I> {
    fn sibling(&self, extension: &str) -> PathBuf {
        let mut name = self.path.as_os_str().to_owned();
        name.push(extension);
        PathBuf::from(name)
    }

    fn decode(bytes: &[u8]) -> Result<bool, DecodeError> {
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
        let file: PreferenceFile =
            serde_json::from_value(value).map_err(|_| DecodeError::Malformed)?;
        Ok(file.enabled)
    }

    /// Refuse to replace either half of a newer writer's recoverable state.
    /// A malformed v1 file can be repaired by an explicit choice, but a schema
    /// this build does not understand is read-only just like newer Store data.
    fn ensure_write_compatible(&mut self) -> Result<(), SchedulerFailure> {
        let paths = [self.path.clone(), self.sibling(".previous")];
        for path in paths {
            let Some(bytes) = self.io.read(&path).map_err(|_| failure())? else {
                continue;
            };
            if matches!(Self::decode(&bytes), Err(DecodeError::FutureSchema)) {
                return Err(failure());
            }
        }
        Ok(())
    }
}

impl<I: PreferenceIo> DeliveryPreferences for FileDeliveryPreferences<I> {
    fn load_enabled(&mut self) -> Result<bool, SchedulerFailure> {
        let primary = self.io.read(&self.path).map_err(|_| failure())?;
        if let Some(bytes) = primary.as_deref() {
            match Self::decode(bytes) {
                Ok(enabled) => return Ok(enabled),
                Err(DecodeError::FutureSchema) => return Err(failure()),
                Err(DecodeError::Malformed) => {}
            }
        }
        let previous = self.sibling(".previous");
        let backup = self.io.read(&previous).map_err(|_| failure())?;
        if let Some(bytes) = backup.as_deref() {
            match Self::decode(bytes) {
                Ok(enabled) => return Ok(enabled),
                Err(DecodeError::FutureSchema | DecodeError::Malformed) => {
                    return Err(failure());
                }
            }
        }
        if primary.is_none() {
            Ok(true)
        } else {
            Err(failure())
        }
    }

    fn set_enabled(&mut self, enabled: bool) -> Result<(), SchedulerFailure> {
        self.ensure_write_compatible()?;
        let part = self.sibling(".part");
        let previous = self.sibling(".previous");
        let bytes = serde_json::to_vec(&PreferenceFile {
            schema_version: SCHEMA_VERSION,
            enabled,
        })
        .map_err(|_| failure())?;
        self.io
            .create_parent(&self.path)
            .and_then(|()| self.io.write_synced(&part, &bytes))
            .map_err(|_| failure())?;
        let staged = self.io.read(&part).map_err(|_| failure())?;
        if staged
            .as_deref()
            .map(Self::decode)
            .transpose()
            .map_err(|_| failure())?
            != Some(enabled)
        {
            return Err(failure());
        }
        self.io.remove(&previous).map_err(|_| failure())?;
        let had_primary = self.io.read(&self.path).map_err(|_| failure())?.is_some();
        if had_primary {
            self.io
                .rename(&self.path, &previous)
                .map_err(|_| failure())?;
        }
        if self.io.rename(&part, &self.path).is_err() {
            if had_primary {
                let _ = self.io.rename(&previous, &self.path);
            }
            return Err(failure());
        }
        Ok(())
    }
}

fn failure() -> SchedulerFailure {
    SchedulerFailure::new(FailureCategory::Preferences)
}

#[cfg(test)]
mod tests {
    use std::collections::HashMap;

    use super::*;

    #[derive(Default)]
    struct FakeIo {
        files: HashMap<PathBuf, Vec<u8>>,
        fail_final_rename: bool,
    }

    impl PreferenceIo for FakeIo {
        fn read(&mut self, path: &Path) -> std::io::Result<Option<Vec<u8>>> {
            Ok(self.files.get(path).cloned())
        }

        fn write_synced(&mut self, path: &Path, bytes: &[u8]) -> std::io::Result<()> {
            self.files.insert(path.to_owned(), bytes.to_vec());
            Ok(())
        }

        fn rename(&mut self, from: &Path, to: &Path) -> std::io::Result<()> {
            if self.fail_final_rename && from.to_string_lossy().ends_with(".part") {
                return Err(std::io::Error::other("rename failed"));
            }
            let bytes = self
                .files
                .remove(from)
                .ok_or_else(|| std::io::Error::other("missing source"))?;
            self.files.insert(to.to_owned(), bytes);
            Ok(())
        }

        fn remove(&mut self, path: &Path) -> std::io::Result<()> {
            self.files.remove(path);
            Ok(())
        }

        fn create_parent(&mut self, _path: &Path) -> std::io::Result<()> {
            Ok(())
        }
    }

    fn preferences(io: FakeIo) -> FileDeliveryPreferences<FakeIo> {
        FileDeliveryPreferences {
            path: PathBuf::from("preferences.json"),
            io,
        }
    }

    #[test]
    fn missing_file_defaults_to_enabled() {
        assert!(preferences(FakeIo::default()).load_enabled().unwrap());
    }

    #[test]
    fn choice_round_trips_without_profile_state() {
        let mut preferences = preferences(FakeIo::default());
        preferences.set_enabled(false).unwrap();
        assert!(!preferences.load_enabled().unwrap());
        preferences.set_enabled(true).unwrap();
        assert!(preferences.load_enabled().unwrap());
    }

    #[test]
    fn failed_replacement_restores_the_previous_choice() {
        let mut preferences = preferences(FakeIo::default());
        preferences.set_enabled(false).unwrap();
        preferences.io.fail_final_rename = true;
        assert!(preferences.set_enabled(true).is_err());
        preferences.io.fail_final_rename = false;
        assert!(!preferences.load_enabled().unwrap());
    }

    #[test]
    fn interrupted_publish_recovers_the_previous_choice() {
        let mut io = FakeIo::default();
        io.files.insert(
            PathBuf::from("preferences.json.previous"),
            serde_json::to_vec(&PreferenceFile {
                schema_version: SCHEMA_VERSION,
                enabled: false,
            })
            .unwrap(),
        );
        assert!(!preferences(io).load_enabled().unwrap());
    }

    #[test]
    fn future_schema_never_reverses_the_choice_to_default() {
        let mut io = FakeIo::default();
        io.files.insert(
            PathBuf::from("preferences.json"),
            br#"{"schema_version":2,"enabled":false}"#.to_vec(),
        );
        assert!(preferences(io).load_enabled().is_err());
    }

    #[test]
    fn future_schema_cannot_be_overwritten_from_primary_or_backup() {
        for path in ["preferences.json", "preferences.json.previous"] {
            let mut io = FakeIo::default();
            let newer = serde_json::to_vec(&PreferenceFile {
                schema_version: SCHEMA_VERSION + 1,
                enabled: false,
            })
            .unwrap();
            io.files.insert(PathBuf::from(path), newer.clone());
            let mut preferences = preferences(io);

            assert!(preferences.set_enabled(true).is_err(), "wrote over {path}");
            assert_eq!(preferences.io.files.get(Path::new(path)), Some(&newer));
            assert!(
                !preferences
                    .io
                    .files
                    .contains_key(Path::new("preferences.json.part"))
            );
        }
    }
}
