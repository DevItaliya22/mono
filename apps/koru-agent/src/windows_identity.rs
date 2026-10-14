// SPDX-License-Identifier: MPL-2.0

//! Per-user registration for the sparse package that owns Windows reminders.
//!
//! The ZIP remains the installer and the updater still swaps files in place.
//! The tiny signed MSIX beside those files supplies only identity. On the first
//! run, and whenever its version changes, the Agent registers it against that
//! stable external directory and restarts once so the new process receives the
//! package identity.

use std::path::Path;

#[cfg(windows)]
use serde::{Deserialize, Serialize};

// The stamp is the Windows registration's; the script below is tested everywhere.
#[cfg(windows)]
const REGISTRATION_STAMP_SCHEMA_VERSION: u8 = 1;

#[cfg(windows)]
#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct RegistrationStamp {
    schema_version: u8,
    package_version: String,
    /// Stable hash of the canonical external location. The device-local stamp
    /// needs to notice a portable move, not disclose the person's path.
    external_location_key: String,
}

#[cfg(windows)]
impl RegistrationStamp {
    fn new(package_version: &str, external_location_key: String) -> Self {
        Self {
            schema_version: REGISTRATION_STAMP_SCHEMA_VERSION,
            package_version: package_version.to_owned(),
            external_location_key,
        }
    }

    fn matches(bytes: &[u8], expected: &Self) -> bool {
        serde_json::from_slice::<Self>(bytes).is_ok_and(|stamp| {
            stamp.schema_version == REGISTRATION_STAMP_SCHEMA_VERSION && stamp == *expected
        })
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum Registration {
    /// This build intentionally carries no package metadata (ordinary cargo
    /// builds), or its identity package is not beside the executable.
    Unavailable,
    /// The matching package version was already registered for this user.
    Current,
    /// Registration changed. The caller must restart before using WinRT APIs.
    Changed,
}

#[cfg(windows)]
pub fn ensure_registered(install_dir: &Path) -> Result<Registration, String> {
    use base64::Engine as _;
    use std::os::windows::process::CommandExt;

    let Some(publisher) = option_env!("KORU_WINDOWS_PACKAGE_PUBLISHER") else {
        return Ok(Registration::Unavailable);
    };
    let Some(version) = option_env!("KORU_WINDOWS_PACKAGE_VERSION") else {
        return Ok(Registration::Unavailable);
    };
    let package = install_dir.join(koru_update::WINDOWS_IDENTITY_PACKAGE);
    if !package.is_file() {
        return Ok(Registration::Unavailable);
    }
    let canonical_install = std::fs::canonicalize(install_dir)
        .map_err(|error| format!("could not resolve Windows package external location: {error}"))?;
    let expected_stamp = RegistrationStamp::new(version, external_location_key(&canonical_install));
    let stamp_path = registration_stamp_path()?;
    let may_be_current = std::fs::read(&stamp_path)
        .ok()
        .is_some_and(|bytes| RegistrationStamp::matches(&bytes, &expected_stamp));
    let identity = koru_winres::notification_identity(koru_channel::segment(), publisher);
    if current_process_package_matches(&identity.name, version) {
        if !may_be_current {
            write_registration_stamp(&stamp_path, &expected_stamp)?;
        }
        return Ok(Registration::Current);
    }
    let script = registration_script(
        &identity.name,
        version,
        &package,
        &canonical_install,
        may_be_current,
    );
    let encoded = base64::engine::general_purpose::STANDARD.encode(
        script
            .encode_utf16()
            .flat_map(u16::to_le_bytes)
            .collect::<Vec<_>>(),
    );
    let output = std::process::Command::new("powershell.exe")
        .args([
            "-NoLogo",
            "-NoProfile",
            "-NonInteractive",
            "-WindowStyle",
            "Hidden",
            "-ExecutionPolicy",
            "Bypass",
            "-EncodedCommand",
            &encoded,
        ])
        .creation_flags(0x0800_0000) // CREATE_NO_WINDOW
        .output()
        .map_err(|error| format!("could not start Windows package registration: {error}"))?;
    if !output.status.success() {
        return Err(format!(
            "Windows package registration failed: {}",
            String::from_utf8_lossy(&output.stderr).trim()
        ));
    }
    match String::from_utf8_lossy(&output.stdout).trim() {
        "current" => Ok(Registration::Current),
        "changed" => {
            write_registration_stamp(&stamp_path, &expected_stamp)?;
            Ok(Registration::Changed)
        }
        output => Err(format!(
            "Windows package registration returned an unexpected result: {output}"
        )),
    }
}

#[cfg(windows)]
fn current_process_package_matches(name: &str, version: &str) -> bool {
    let Ok(package) = windows::ApplicationModel::Package::Current() else {
        return false;
    };
    let Ok(id) = package.Id() else {
        return false;
    };
    let Ok(current_name) = id.Name() else {
        return false;
    };
    let Ok(current_version) = id.Version() else {
        return false;
    };
    package_identity_fields_match(name, version, &current_name.to_string(), current_version)
}

#[cfg(windows)]
fn package_identity_fields_match(
    expected_name: &str,
    expected_version: &str,
    current_name: &str,
    current_version: windows::ApplicationModel::PackageVersion,
) -> bool {
    current_name == expected_name
        && format!(
            "{}.{}.{}.{}",
            current_version.Major,
            current_version.Minor,
            current_version.Build,
            current_version.Revision
        ) == expected_version
}

#[cfg(not(windows))]
pub fn ensure_registered(_install_dir: &Path) -> Result<Registration, String> {
    Ok(Registration::Unavailable)
}

#[cfg(any(windows, test))]
fn registration_script(
    name: &str,
    version: &str,
    package: &Path,
    install_dir: &Path,
    may_be_current: bool,
) -> String {
    let name = powershell_literal(name);
    let version = powershell_literal(version);
    let package = powershell_literal(&package.display().to_string());
    let install_dir = powershell_literal(&install_dir.display().to_string());
    let may_be_current = if may_be_current { "$true" } else { "$false" };
    format!(
        "$ErrorActionPreference='Stop';\n\
         $match=Get-AppxPackage -Name {name} | Where-Object {{ $_.Version -eq [version]{version} }} | Select-Object -First 1;\n\
         if ({may_be_current} -and $null -ne $match) {{ Write-Output 'current'; exit 0 }};\n\
         Add-AppxPackage -Path {package} -ExternalLocation {install_dir} -ForceUpdateFromAnyVersion;\n\
         Write-Output 'changed'"
    )
}

#[cfg(windows)]
fn registration_stamp_path() -> Result<std::path::PathBuf, String> {
    koru_channel::channel_root()
        .map(|root| root.join("agent/windows-notification-registration-v1.json"))
        .ok_or_else(|| "could not locate the channel root for Windows package registration".into())
}

#[cfg(windows)]
fn external_location_key(path: &Path) -> String {
    use std::os::windows::ffi::OsStrExt as _;

    let mut hash: u64 = 0xcbf2_9ce4_8422_2325;
    for unit in path.as_os_str().encode_wide() {
        for byte in unit.to_le_bytes() {
            let byte = if byte.is_ascii_uppercase() {
                byte.to_ascii_lowercase()
            } else {
                byte
            };
            hash ^= u64::from(byte);
            hash = hash.wrapping_mul(0x0000_0100_0000_01b3);
        }
    }
    format!("{hash:016x}")
}

#[cfg(windows)]
fn write_registration_stamp(path: &Path, stamp: &RegistrationStamp) -> Result<(), String> {
    use std::io::Write as _;

    let parent = path
        .parent()
        .ok_or_else(|| "Windows package registration stamp has no parent".to_string())?;
    std::fs::create_dir_all(parent)
        .map_err(|error| format!("could not create Windows package stamp directory: {error}"))?;
    let staged = path.with_extension(format!("json.{}.part", std::process::id()));
    let result = (|| {
        let mut file = std::fs::File::create(&staged)
            .map_err(|error| format!("could not stage Windows package registration: {error}"))?;
        serde_json::to_writer(&mut file, stamp)
            .map_err(|error| format!("could not encode Windows package registration: {error}"))?;
        file.write_all(b"\n")
            .and_then(|()| file.sync_all())
            .map_err(|error| format!("could not sync Windows package registration: {error}"))?;
        replace_file(&staged, path)
    })();
    if result.is_err() {
        let _ = std::fs::remove_file(staged);
    }
    result
}

#[cfg(windows)]
fn replace_file(source: &Path, target: &Path) -> Result<(), String> {
    use std::os::windows::ffi::OsStrExt as _;
    use windows_sys::Win32::Storage::FileSystem::{
        MOVEFILE_REPLACE_EXISTING, MOVEFILE_WRITE_THROUGH, MoveFileExW,
    };

    let source: Vec<u16> = source.as_os_str().encode_wide().chain(Some(0)).collect();
    let target: Vec<u16> = target.as_os_str().encode_wide().chain(Some(0)).collect();
    let moved = unsafe {
        MoveFileExW(
            source.as_ptr(),
            target.as_ptr(),
            MOVEFILE_REPLACE_EXISTING | MOVEFILE_WRITE_THROUGH,
        )
    };
    if moved == 0 {
        Err(format!(
            "could not publish Windows package registration: {}",
            std::io::Error::last_os_error()
        ))
    } else {
        Ok(())
    }
}

#[cfg(any(windows, test))]
fn powershell_literal(value: &str) -> String {
    format!("'{}'", value.replace('\'', "''"))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn registration_script_is_literal_and_version_gated() {
        let script = registration_script(
            "Koru.Desktop.Beta",
            "1.2.3.42",
            Path::new("C:/People/O'Brien/Koru/koru-identity.msix"),
            Path::new("C:/People/O'Brien/Koru"),
            true,
        );
        assert!(script.contains("Get-AppxPackage -Name 'Koru.Desktop.Beta'"));
        assert!(script.contains("[version]'1.2.3.42'"));
        assert!(script.contains("O''Brien"));
        assert!(script.contains("-ExternalLocation 'C:/People/O''Brien/Koru'"));
        assert!(script.contains("-ForceUpdateFromAnyVersion"));
        assert!(script.contains("if ($true -and $null -ne $match)"));
    }

    #[cfg(windows)]
    #[test]
    fn the_same_package_version_re_registers_after_a_portable_path_move() {
        let old =
            RegistrationStamp::new("1.2.3.42", external_location_key(Path::new(r"C:\old\Koru")));
        let bytes = serde_json::to_vec(&old).unwrap();
        let same =
            RegistrationStamp::new("1.2.3.42", external_location_key(Path::new(r"c:\old\Koru")));
        let moved = RegistrationStamp::new(
            "1.2.3.42",
            external_location_key(Path::new(r"D:\portable\Koru")),
        );

        assert!(RegistrationStamp::matches(&bytes, &same));
        assert!(!RegistrationStamp::matches(&bytes, &moved));
        let script = registration_script(
            "Koru.Desktop.Beta",
            "1.2.3.42",
            Path::new(r"D:\portable\Koru\koru-identity.msix"),
            Path::new(r"D:\portable\Koru"),
            false,
        );
        assert!(script.contains("if ($false -and $null -ne $match)"));
    }

    #[cfg(windows)]
    #[test]
    fn current_identity_requires_the_exact_name_and_four_part_version() {
        let version = windows::ApplicationModel::PackageVersion {
            Major: 1,
            Minor: 2,
            Build: 3,
            Revision: 42,
        };
        assert!(package_identity_fields_match(
            "Koru.Desktop.Beta",
            "1.2.3.42",
            "Koru.Desktop.Beta",
            version,
        ));
        assert!(!package_identity_fields_match(
            "Koru.Desktop.Beta",
            "1.2.3.43",
            "Koru.Desktop.Beta",
            version,
        ));
        assert!(!package_identity_fields_match(
            "Koru.Desktop.Beta",
            "1.2.3.42",
            "Koru.Desktop.Nightly",
            version,
        ));
    }
}
