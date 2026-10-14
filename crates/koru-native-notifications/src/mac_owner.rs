// SPDX-License-Identifier: MPL-2.0

//! Bounded private protocol between the Agent and the signed macOS
//! notification-owner app. It is platform-independent so its shape and
//! activation validation remain testable on every development machine.

use serde::{Deserialize, Serialize};
use std::ffi::OsString;
use std::path::{Path, PathBuf};

#[cfg(any(target_os = "macos", test))]
use crate::DeliverySupport;
use crate::{
    Action, ActionSet, Activation, Authorization, Content, Failure, RegistrationId, Sound,
};

pub const MAC_OWNER_BUNDLE_ID: &str = "do.koru.desktop.agent.notifications";
pub const MAC_OWNER_APP: &str = "Koru Agent.app";
pub const MAC_OWNER_EXECUTABLE: &str = "koru-notification-owner";
pub const MAC_CATEGORY_REMINDER: &str = "koru.reminder.v1";
pub const MAC_CATEGORY_OPEN_ONLY: &str = "koru.reminder.open-only.v1";
pub const MAC_ACTION_OPEN: &str = "koru.reminder.open";
pub const MAC_ACTION_SNOOZE: &str = "koru.reminder.snooze";
pub const MAC_ACTION_HANDLED: &str = "koru.reminder.handled";
pub const MAX_MAC_OWNER_MESSAGE: usize = 16 * 1024;
pub const MAX_MAC_ROOT_LEN: usize = 4096;

#[cfg(any(target_os = "macos", test))]
pub(crate) fn mac_category(actions: ActionSet) -> &'static str {
    match actions {
        ActionSet::Reminder => MAC_CATEGORY_REMINDER,
        ActionSet::OpenOnly => MAC_CATEGORY_OPEN_ONLY,
    }
}

/// The private route persisted by the notification owner for one native
/// namespace. `Default` is a value in its own right: a missing or unreadable
/// route must never be mistaken for permission to open the default Agent.
#[derive(Clone, PartialEq, Eq, Serialize, Deserialize, Debug)]
#[serde(
    tag = "kind",
    content = "root",
    rename_all = "snake_case",
    deny_unknown_fields
)]
pub enum MacRootRoute {
    Default,
    Root(String),
}

impl MacRootRoute {
    pub fn from_explicit_root(root: Option<&Path>) -> Result<Self, InvalidMacRootRoute> {
        let Some(root) = root else {
            return Ok(Self::Default);
        };
        let root = root.to_str().ok_or(InvalidMacRootRoute)?;
        let route = Self::Root(root.to_owned());
        route.validate()?;
        Ok(route)
    }

    pub fn explicit_root(&self) -> Option<&str> {
        match self {
            Self::Default => None,
            Self::Root(root) => Some(root),
        }
    }

    pub fn validate(&self) -> Result<(), InvalidMacRootRoute> {
        if let Self::Root(root) = self
            && (root.is_empty()
                || root.len() > MAX_MAC_ROOT_LEN
                || root.contains(['\0', '\n', '\r']))
        {
            return Err(InvalidMacRootRoute);
        }
        Ok(())
    }
}

/// Decode one route file. Absence is an error, not the default route.
pub fn decode_mac_root_route(bytes: Option<&[u8]>) -> Result<MacRootRoute, InvalidMacRootRoute> {
    let bytes = bytes.ok_or(InvalidMacRootRoute)?;
    let route: MacRootRoute = serde_json::from_slice(bytes).map_err(|_| InvalidMacRootRoute)?;
    route.validate()?;
    Ok(route)
}

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub struct InvalidMacRootRoute;

#[derive(Clone, PartialEq, Eq, Serialize, Deserialize, Debug)]
#[serde(tag = "operation", rename_all = "snake_case", deny_unknown_fields)]
pub enum MacOwnerRequest {
    Inspect,
    Schedule {
        native_id: String,
        deliver_at_unix_ms: i64,
        content: Content,
        sound: Sound,
        actions: ActionSet,
        root_route: MacRootRoute,
    },
    Withdraw {
        native_id: String,
    },
    ResetNamespace {
        prefix: String,
    },
    OpenSettings,
    RequestAuthorization,
    SendTest {
        native_id: String,
        content: Content,
        sound: Sound,
    },
}

#[derive(Clone, PartialEq, Eq, Serialize, Deserialize, Debug)]
#[serde(rename_all = "snake_case")]
pub enum MacOwnerValue {
    Authorization(Authorization),
    Done,
}

pub type MacOwnerResponse = Result<MacOwnerValue, Failure>;

#[derive(Clone, PartialEq, Eq, Debug)]
pub struct MacActionTitles {
    pub open: String,
    pub snooze: String,
    pub handled: String,
}

#[derive(Clone, PartialEq, Eq, Debug)]
pub struct MacActivationLaunch {
    pub activation: Activation,
    pub explicit_root: Option<String>,
}

pub fn activation_from_mac_response(
    native_id: &str,
    action_identifier: &str,
    root_route: MacRootRoute,
) -> Option<MacActivationLaunch> {
    let action = match action_identifier {
        MAC_ACTION_OPEN | "com.apple.UNNotificationDefaultActionIdentifier" => Action::Open,
        MAC_ACTION_SNOOZE => Action::Snooze,
        MAC_ACTION_HANDLED => Action::Handled,
        // Dismiss and unknown identifiers are deliberately not synchronized.
        _ => return None,
    };
    let (_, registration) = parse_native_id(native_id)?;
    root_route.validate().ok()?;
    Some(MacActivationLaunch {
        activation: Activation::new(registration, action),
        explicit_root: root_route.explicit_root().map(str::to_owned),
    })
}

pub fn namespace_from_native_id(native_id: &str) -> Option<&str> {
    parse_native_id(native_id).map(|(namespace, _)| namespace)
}

/// Parse the whole native identifier, including the complete registration.
fn parse_native_id(native_id: &str) -> Option<(&str, RegistrationId)> {
    let mut pieces = native_id.splitn(3, '.');
    (pieces.next()? == "koru").then_some(())?;
    let namespace = pieces.next()?;
    (namespace.len() == 16
        && namespace
            .bytes()
            .all(|byte| byte.is_ascii_digit() || matches!(byte, b'a'..=b'f')))
    .then_some(())?;
    let registration = RegistrationId::new(pieces.next()?).ok()?;
    Some((namespace, registration))
}

pub fn mac_owner_executable_from(agent_executable: &Path) -> Option<PathBuf> {
    let macos = agent_executable.parent()?;
    let contents = macos.parent()?;
    Some(
        contents
            .join("Library/LoginItems")
            .join(MAC_OWNER_APP)
            .join("Contents/MacOS")
            .join(MAC_OWNER_EXECUTABLE),
    )
}

/// Resolve the outer Agent from the exact nested-owner package geometry.
pub fn mac_agent_executable_from_owner(owner_executable: &Path) -> Option<PathBuf> {
    (owner_executable.file_name()? == MAC_OWNER_EXECUTABLE).then_some(())?;
    let macos = owner_executable.parent()?;
    (macos.file_name()? == "MacOS").then_some(())?;
    let contents = macos.parent()?;
    (contents.file_name()? == "Contents").then_some(())?;
    let owner_app = contents.parent()?;
    (owner_app.file_name()? == MAC_OWNER_APP).then_some(())?;
    let login_items = owner_app.parent()?;
    (login_items.file_name()? == "LoginItems").then_some(())?;
    let library = login_items.parent()?;
    (library.file_name()? == "Library").then_some(())?;
    let outer_contents = library.parent()?;
    (outer_contents.file_name()? == "Contents").then_some(())?;
    Some(outer_contents.join("MacOS/koru-agent"))
}

/// Resolve the Info.plist belonging to the exact nested owner executable.
pub fn mac_owner_info_plist_from(owner_executable: &Path) -> Option<PathBuf> {
    mac_agent_executable_from_owner(owner_executable)?;
    Some(owner_executable.parent()?.parent()?.join("Info.plist"))
}

/// Convert package inspection into the static support claimed by the adapter.
/// Kept pure so an unpackaged build cannot accidentally grow one true field.
#[cfg(any(target_os = "macos", test))]
pub(crate) fn mac_delivery_support(
    owner_installed: bool,
    bundled_sounds_installed: bool,
) -> DeliverySupport {
    DeliverySupport {
        actions: owner_installed,
        stable_replacement: owner_installed,
        withdrawal: owner_installed,
        custom_sound: owner_installed && bundled_sounds_installed,
        activation_while_stopped: owner_installed,
    }
}

/// One private route publication attempt. A time seed plus process-local nonce
/// means a stale temp file from a crash or reused pid cannot block all writes.
#[cfg(any(target_os = "macos", test))]
pub(crate) fn mac_route_temp_name(
    namespace: &str,
    process_id: u32,
    seed: u128,
    nonce: u64,
) -> String {
    format!("{namespace}.part-{process_id}-{seed:x}-{nonce:x}")
}

/// Exact argv used for the existing Agent bootstrap.
pub fn mac_agent_launch_args(launch: &MacActivationLaunch) -> Vec<OsString> {
    let mut args = vec![
        OsString::from("notification-activation"),
        OsString::from(launch.activation.encode_opaque()),
    ];
    if let Some(root) = &launch.explicit_root {
        args.push(OsString::from("--root"));
        args.push(OsString::from(root));
    }
    args
}

impl std::fmt::Display for InvalidMacRootRoute {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter.write_str("macOS notification root route is invalid")
    }
}

impl std::error::Error for InvalidMacRootRoute {}
