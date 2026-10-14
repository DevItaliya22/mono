// SPDX-License-Identifier: MPL-2.0

//! A person's quality choice, shared by the client, worker, and model catalog.

use serde::{Deserialize, Serialize};

/// A stable quality preference, never a model id.
#[derive(Copy, Clone, PartialEq, Eq, Hash, Debug, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ModelTier {
    Compact,
    HighAccuracy,
}

impl ModelTier {
    pub const ALL: [ModelTier; 2] = [ModelTier::Compact, ModelTier::HighAccuracy];

    pub fn as_str(self) -> &'static str {
        match self {
            ModelTier::Compact => "compact",
            ModelTier::HighAccuracy => "high_accuracy",
        }
    }

    pub fn parse(token: &str) -> Option<ModelTier> {
        ModelTier::ALL
            .into_iter()
            .find(|tier| tier.as_str() == token)
    }
}

impl Default for ModelTier {
    fn default() -> ModelTier {
        ModelTier::Compact
    }
}

impl std::fmt::Display for ModelTier {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(self.as_str())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn stable_tokens_round_trip() {
        for tier in ModelTier::ALL {
            assert_eq!(ModelTier::parse(tier.as_str()), Some(tier));
            assert_eq!(
                serde_json::to_string(&tier).unwrap(),
                format!("\"{}\"", tier.as_str())
            );
        }
    }

    #[test]
    fn compact_is_the_default() {
        assert_eq!(ModelTier::default(), ModelTier::Compact);
    }
}
