// SPDX-License-Identifier: MPL-2.0

use koru_store_core::ids::ProfileId;
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};

/// Minimal local inventory used to reconcile hosted Profile lifecycle without
/// exporting Profile names, appearance, purpose, locale, or content.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct ProfileLifecycleInventoryItem {
    pub profile_id: ProfileId,
    pub account_binding: String,
    #[serde(default)]
    pub removed_at_millis: Option<i64>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct ProfileLifecycleInventory {
    pub profiles: Vec<ProfileLifecycleInventoryItem>,
}
