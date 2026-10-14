// SPDX-License-Identifier: MPL-2.0

//! Provider adapter registry.
//!
//! Real provider SDKs remain confined to this crate. The first implementation
//! uses the same registry with a deterministic mock adapter.

use std::collections::HashMap;
use std::sync::Arc;

use koru_ai_core::{AiError, AiProvider, AiProviderId, AiRequest, Cancellation, ProviderStream};

mod credentials;
mod mock;
mod runtime;

pub use credentials::{
    CredentialStore, OPENAI_VAULT_SERVICE, OsCredentialStore, SecretString, SessionCredentialStore,
};
pub use koru_ai_core::validate_reasoning;
pub use koru_ai_provider_koru::{KORU_MODEL, KORU_MODEL_LABEL, KoruProvider, koru_model};
pub use koru_ai_provider_ollama::{ModelPullStream, OllamaProvider, OllamaService, ollama_model};
pub use koru_ai_provider_openai::{OpenAiProvider, openai_model};
pub use mock::DeterministicMockProvider;
pub use runtime::ProviderRuntime;

#[derive(Default)]
pub struct ProviderRegistry {
    providers: HashMap<AiProviderId, Arc<dyn AiProvider>>,
}

impl ProviderRegistry {
    pub fn insert(&mut self, id: AiProviderId, provider: Arc<dyn AiProvider>) {
        self.providers.insert(id, provider);
    }

    pub fn contains(&self, id: &AiProviderId) -> bool {
        self.providers.contains_key(id)
    }

    pub fn is_empty(&self) -> bool {
        self.providers.is_empty()
    }

    pub fn ids(&self) -> impl Iterator<Item = &AiProviderId> {
        self.providers.keys()
    }

    pub fn start(
        &self,
        request: AiRequest,
        cancellation: Cancellation,
    ) -> Result<ProviderStream, AiError> {
        let provider = self.providers.get(&request.provider).ok_or_else(|| {
            AiError::new(
                koru_ai_core::AiErrorClass::Configuration,
                "The selected AI provider is not configured.",
                false,
            )
        })?;
        provider.start(request, cancellation)
    }
}
