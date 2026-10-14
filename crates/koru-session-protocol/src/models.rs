// SPDX-License-Identifier: MPL-2.0

//! Private display metadata from a provider-owned model catalog.
use crate::{ModelName, TokenParseError};
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};

pub const MAX_PROVIDER_MODELS: usize = 4096;

/// A complete bounded catalog. Empty means the provider exposes no choices.
/// IDs remain opaque; grouping is presentation supplied by the adapter.
#[derive(Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(try_from = "Vec<ProviderModel>", into = "Vec<ProviderModel>")]
pub struct ProviderModels(Vec<ProviderModel>);

#[derive(Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct ProviderModel {
    pub id: ModelName,
    pub group: ModelName,
}

impl ProviderModels {
    pub fn models(&self) -> &[ProviderModel] {
        &self.0
    }
}
impl TryFrom<Vec<ProviderModel>> for ProviderModels {
    type Error = TokenParseError;
    fn try_from(models: Vec<ProviderModel>) -> Result<Self, Self::Error> {
        let ids: std::collections::BTreeSet<_> = models.iter().map(|model| &model.id).collect();
        if models.len() > MAX_PROVIDER_MODELS || ids.len() != models.len() {
            return Err(TokenParseError("invalid model catalog"));
        }
        Ok(Self(models))
    }
}
impl From<ProviderModels> for Vec<ProviderModel> {
    fn from(value: ProviderModels) -> Self {
        value.0
    }
}
impl std::fmt::Debug for ProviderModels {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("ProviderModels")
            .field("count", &self.0.len())
            .finish()
    }
}
impl std::fmt::Debug for ProviderModel {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str("ProviderModel(<redacted>)")
    }
}

#[cfg(test)]
mod tests;
