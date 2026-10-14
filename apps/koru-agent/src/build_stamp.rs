// SPDX-License-Identifier: MPL-2.0

//! What this device tells its peers it runs — the payload of the build record
//! the agent signs at startup ([`crate::app::App::advertise_build`]).
//!
//! The agent, not `koru.exe`, is the authority, for the reason
//! `koru_agent_wire::Push::Updated` gives: after an update the window and the
//! agent can be different builds, and the agent is the process that owns and
//! signs the trust store.

use koru_device::BuildPayload;

pub fn payload() -> BuildPayload {
    let info = koru_channel::build_info();
    let (os, os_version) = os_info();
    BuildPayload {
        app_version: info.version.to_owned(),
        channel: info.channel.to_owned(),
        branch: branch().map(str::to_owned),
        commit: commit(info.commit).map(str::to_owned),
        os: Some(os),
        os_version: Some(os_version),
        peer_protocol: Some(koru_sync_control::PEER_PROTOCOL_VERSION),
    }
}

/// Stamped by this crate's build script for a cargo build, and deliberately
/// absent from a released one — the same reader the tray tooltip uses.
fn branch() -> Option<&'static str> {
    option_env!("KORU_BRANCH")
        .map(str::trim)
        .filter(|name| !name.is_empty())
}

/// The commit this agent was built from: the packager's stamp on a released
/// build, this crate's own build-script stamp on an internal one (the
/// `koru-channel` crate cannot see the latter, which is why `build_info`'s
/// copy is only the fall-back). `None` when neither knows.
fn commit(from_build_info: &'static str) -> Option<&'static str> {
    option_env!("KORU_COMMIT")
        .or(Some(from_build_info))
        .map(str::trim)
        .filter(|commit| !commit.is_empty() && *commit != "unknown")
}

/// `("Windows 11", "10.0.26200")`: the short form for a list row, the full
/// one for a log line. `RtlGetVersion` rather than `GetVersionExW`, which
/// lies to a process without a compatibility manifest.
#[cfg(windows)]
fn os_info() -> (String, String) {
    use windows_sys::Wdk::System::SystemServices::RtlGetVersion;
    use windows_sys::Win32::System::SystemInformation::OSVERSIONINFOW;

    // SAFETY: a zeroed OSVERSIONINFOW with its size set is the documented
    // input; RtlGetVersion only writes inside it.
    let mut info: OSVERSIONINFOW = unsafe { std::mem::zeroed() };
    info.dwOSVersionInfoSize = std::mem::size_of::<OSVERSIONINFOW>() as u32;
    let status = unsafe { RtlGetVersion(&mut info) };
    if status != 0 {
        return ("Windows".into(), "unknown".into());
    }
    // Windows 11 reports major 10; the build number is what tells them apart.
    let name = if info.dwMajorVersion >= 10 && info.dwBuildNumber >= 22000 {
        "Windows 11"
    } else if info.dwMajorVersion >= 10 {
        "Windows 10"
    } else {
        "Windows"
    };
    (
        name.into(),
        format!(
            "{}.{}.{}",
            info.dwMajorVersion, info.dwMinorVersion, info.dwBuildNumber
        ),
    )
}

/// `("macOS 26.0", "26.0.1 (25A362)")`: the product version the way About
/// This Mac prints it, and the Darwin build beside the patch for a log line.
///
/// **The reading is `koru_window::system_version`’s, and the sentences are
/// this function’s.** Both were here — a `sysctlbyname` helper and the
/// formatting — until `koru-workspace-host`’s environment collector needed the same
/// two numbers and could not see this crate. What went down is the mechanism;
/// what stayed is the knowledge that a device row says "macOS 26.0" and a log
/// line wants the build in parentheses (ADR-173).
#[cfg(target_os = "macos")]
fn os_info() -> (String, String) {
    let Some(version) = koru_window::system_version::system_version() else {
        return ("macOS".into(), "unknown".into());
    };
    let short = version
        .product
        .splitn(3, '.')
        .take(2)
        .collect::<Vec<_>>()
        .join(".");
    let full = match version.build {
        Some(build) => format!("{} ({build})", version.product),
        None => version.product,
    };
    (format!("macOS {short}"), full)
}

#[cfg(not(any(windows, target_os = "macos")))]
fn os_info() -> (String, String) {
    let os = std::env::consts::OS;
    (os.to_owned(), os.to_owned())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_payload_is_within_the_record_limits_and_names_the_os() {
        let payload = payload();
        assert!(!payload.app_version.is_empty());
        assert!(!payload.channel.is_empty());
        let os = payload.os.as_deref().unwrap();
        assert!(
            os.starts_with("Windows") || os.starts_with("macOS ") || os == std::env::consts::OS,
            "{os}"
        );
        // Must sign and verify as a record, which is where the limits bite.
        let key = koru_device::DeviceSigningKey::generate();
        let record = koru_device::DeviceBuildV1::sign(
            koru_device::RecordId::new(),
            koru_device::DeviceId::new(),
            &payload,
            1,
            &key,
        );
        record
            .verify(key.public_key())
            .expect("the stamp this agent signs verifies");
    }

    /// The row says "macOS 26.0", never a Darwin number, and the log line
    /// carries the build in parentheses — About This Mac's two facts.
    #[cfg(target_os = "macos")]
    #[test]
    fn the_mac_os_line_is_the_product_version() {
        let (short, full) = os_info();
        let version = short.strip_prefix("macOS ").unwrap();
        assert_eq!(version.split('.').count(), 2, "{short}");
        assert!(
            version.split('.').all(|part| part.parse::<u32>().is_ok()),
            "{short}"
        );
        assert!(full.starts_with(version), "{full} / {short}");
        assert!(full.ends_with(')'), "{full}");
    }
}
