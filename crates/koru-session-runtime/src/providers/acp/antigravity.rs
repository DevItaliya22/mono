// SPDX-License-Identifier: MPL-2.0

//! Google's official Antigravity ACP server (`agy_acp_server.exe`, AGENTUX I9).
//! It prints no version, so the executable is identified by its SHA-256, and
//! the version its `initialize` names must be the one that digest is pinned to.
use std::ffi::OsStr;
use std::io::Read;

use sha2::{Digest, Sha256};

use super::*;

/// The name the server's `initialize` gives itself in `agentInfo`.
const AGENT_NAME: &str = "antigravity-acp";

/// The lowercase hex SHA-256 of the executable a route would launch. A path
/// that does not resolve is a missing executable, never a rejected version.
pub(super) fn executable_digest(executable: &OsStr) -> Result<String, ProviderFailure> {
    let path = executable
        .to_str()
        .and_then(koru_session_profiles::resolve_executable)
        .ok_or_else(|| {
            failure(
                ErrorCode::ProviderUnavailable,
                ProviderStage::Discovery,
                ProviderDiagnostic::MissingExecutable,
            )
        })?;
    std::fs::File::open(path)
        .and_then(sha256_hex)
        .map_err(|error| ProviderFailure::io(ProviderStage::Version, &error))
}

pub(super) fn sha256_hex(mut reader: impl Read) -> std::io::Result<String> {
    let mut hasher = Sha256::new();
    let mut buffer = vec![0u8; 1 << 20];
    loop {
        let read = reader.read(&mut buffer)?;
        if read == 0 {
            break;
        }
        hasher.update(&buffer[..read]);
    }
    Ok(hasher
        .finalize()
        .iter()
        .map(|byte| format!("{byte:02x}"))
        .collect())
}

/// The server must name itself and the exact version its digest was pinned
/// to. Only the two fixed facts are read; nothing else in `agentInfo` is kept.
pub(super) fn validate_handshake(
    initialized: &Value,
    version: &ProviderVersion,
) -> Result<(), ProviderFailure> {
    let info = initialized.get("agentInfo");
    let named = info
        .and_then(|info| info.get("name"))
        .and_then(Value::as_str)
        == Some(AGENT_NAME);
    let versioned = info
        .and_then(|info| info.get("version"))
        .and_then(Value::as_str)
        == Some(version.as_str());
    if named && versioned {
        Ok(())
    } else {
        Err(failure(
            ErrorCode::ProviderIncompatible,
            ProviderStage::Version,
            ProviderDiagnostic::VersionRejected,
        ))
    }
}
