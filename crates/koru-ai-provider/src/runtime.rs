// SPDX-License-Identifier: MPL-2.0

//! Dynamically configured production providers behind the stable Koru port.

use std::sync::{Arc, RwLock};

use koru_ai_core::{
    AiError, AiErrorClass, AiProvider, AiProviderId, AiRequest, AiSettings, Cancellation,
    ModelDescriptor, ProviderStream, ReasoningChoice,
};

use crate::{
    CredentialStore, KoruProvider, ModelPullStream, OllamaProvider, OllamaService, OpenAiProvider,
    OsCredentialStore, ProviderRegistry, SecretString, validate_reasoning,
};

#[derive(Clone)]
pub struct ProviderRuntime {
    settings: Arc<RwLock<Option<AiSettings>>>,
    credentials: Arc<dyn CredentialStore>,
    openai_account: Arc<str>,
}

impl ProviderRuntime {
    pub fn with_os_vault(channel: &str) -> Result<Self, AiError> {
        Self::new(None, Arc::new(OsCredentialStore), format!("koru-{channel}"))
    }

    pub fn new(
        settings: Option<AiSettings>,
        credentials: Arc<dyn CredentialStore>,
        openai_account: impl Into<String>,
    ) -> Result<Self, AiError> {
        let openai_account = openai_account.into();
        if openai_account.trim().is_empty()
            || openai_account.len() > 200
            || openai_account.chars().any(char::is_control)
        {
            return Err(configuration(
                "The OpenAI credential account label is invalid.",
            ));
        }
        Ok(Self {
            settings: Arc::new(RwLock::new(settings)),
            credentials,
            openai_account: openai_account.into(),
        })
    }

    pub fn registry(&self) -> ProviderRegistry {
        let mut registry = ProviderRegistry::default();
        registry.insert(
            AiProviderId::parse(AiProviderId::OPENAI).expect("built-in provider id"),
            Arc::new(RuntimeOpenAi(self.clone())),
        );
        registry.insert(
            AiProviderId::parse(AiProviderId::OLLAMA).expect("built-in provider id"),
            Arc::new(RuntimeOllama(self.clone())),
        );
        registry.insert(
            AiProviderId::parse(AiProviderId::KORU).expect("built-in provider id"),
            Arc::new(RuntimeKoru(self.clone())),
        );
        registry
    }

    pub fn settings(&self) -> Option<AiSettings> {
        self.settings
            .read()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
            .clone()
    }

    pub fn replace_settings(&self, settings: Option<AiSettings>) {
        *self
            .settings
            .write()
            .unwrap_or_else(|poisoned| poisoned.into_inner()) = settings;
    }

    pub fn openai_account(&self) -> &str {
        &self.openai_account
    }

    pub fn save_openai_secret(&self, secret: SecretString) -> Result<(), AiError> {
        self.credentials.save_openai(&self.openai_account, secret)
    }

    /// Re-check the deterministic desktop vault entry without exposing its
    /// account label or secret through a public DTO.
    pub fn openai_credential_configured(&self) -> bool {
        self.credentials
            .resolve_openai(&self.openai_account)
            .is_ok()
    }

    pub fn remove_openai_secret(&self) -> Result<(), AiError> {
        self.credentials.remove_openai(&self.openai_account)
    }

    pub fn openai_models(&self) -> Result<Vec<String>, AiError> {
        self.openai()?.list_models()
    }

    pub fn validate_openai(
        &self,
        model: &str,
        reasoning: ReasoningChoice,
    ) -> Result<ModelDescriptor, AiError> {
        let descriptor = self.openai()?.validate_model(model)?;
        validate_reasoning(&descriptor.capabilities.reasoning, reasoning)?;
        Ok(descriptor)
    }

    pub fn ollama_models(&self) -> Result<Vec<String>, AiError> {
        self.ollama_service()?.list_models()
    }

    pub fn validate_ollama(
        &self,
        model: &str,
        reasoning: ReasoningChoice,
    ) -> Result<ModelDescriptor, AiError> {
        let descriptor = self.ollama_service()?.inspect_model(model)?;
        validate_reasoning(&descriptor.capabilities.reasoning, reasoning)?;
        Ok(descriptor)
    }

    pub fn koru_models(&self) -> Result<Vec<String>, AiError> {
        self.koru()?.list_models()
    }

    pub fn validate_koru(&self, reasoning: ReasoningChoice) -> Result<ModelDescriptor, AiError> {
        let descriptor = self.koru()?.validate_hosted_model()?;
        validate_reasoning(&descriptor.capabilities.reasoning, reasoning)?;
        Ok(descriptor)
    }

    fn koru(&self) -> Result<KoruProvider, AiError> {
        KoruProvider::from_env()
    }

    pub fn pull_ollama(
        &self,
        model: &str,
        cancellation: Cancellation,
    ) -> Result<ModelPullStream, AiError> {
        self.ollama_service()?.pull_model(model, cancellation)
    }

    fn openai(&self) -> Result<OpenAiProvider, AiError> {
        let settings = self.configured_settings()?;
        let account = settings
            .openai
            .key_account
            .ok_or_else(|| configuration("No OpenAI project API key is configured for Koru."))?;
        let secret = self.credentials.resolve_openai(&account)?;
        Ok(OpenAiProvider::new(secret.into_exposed()))
    }

    fn ollama_service(&self) -> Result<OllamaService, AiError> {
        let settings = self.configured_settings()?;
        Ok(OllamaService::new(settings.ollama.endpoint))
    }

    fn configured_settings(&self) -> Result<AiSettings, AiError> {
        self.settings()
            .ok_or_else(|| configuration("AI provider settings have not been configured."))
    }
}

struct RuntimeOpenAi(ProviderRuntime);

impl AiProvider for RuntimeOpenAi {
    fn start(
        &self,
        request: AiRequest,
        cancellation: Cancellation,
    ) -> Result<ProviderStream, AiError> {
        self.0.openai()?.start(request, cancellation)
    }
}

struct RuntimeOllama(ProviderRuntime);

impl AiProvider for RuntimeOllama {
    fn start(
        &self,
        request: AiRequest,
        cancellation: Cancellation,
    ) -> Result<ProviderStream, AiError> {
        let service = self.0.ollama_service()?;
        OllamaProvider::from_service(service).start(request, cancellation)
    }
}

struct RuntimeKoru(ProviderRuntime);

impl AiProvider for RuntimeKoru {
    fn start(
        &self,
        request: AiRequest,
        cancellation: Cancellation,
    ) -> Result<ProviderStream, AiError> {
        self.0.koru()?.start(request, cancellation)
    }
}

fn configuration(message: &'static str) -> AiError {
    AiError::new(AiErrorClass::Configuration, message, false)
}

#[cfg(test)]
mod tests {
    use koru_ai_core::{AiProviderId, ReasoningChoice, Timestamp};

    use crate::SessionCredentialStore;

    use super::*;

    #[test]
    fn registry_is_stable_while_settings_change_underneath_it() {
        let runtime = ProviderRuntime::new(
            None,
            Arc::new(SessionCredentialStore::default()),
            "test-project",
        )
        .unwrap();
        let registry = runtime.registry();
        assert!(registry.contains(&AiProviderId::parse("openai").unwrap()));
        assert!(registry.contains(&AiProviderId::parse("ollama").unwrap()));
        assert!(registry.contains(&AiProviderId::parse("koru").unwrap()));
        assert!(runtime.ollama_models().is_err());

        let settings = AiSettings::unconfigured(Timestamp::now());
        runtime.replace_settings(Some(settings.clone()));
        assert_eq!(runtime.settings(), Some(settings));
    }

    #[test]
    fn credential_account_is_internal_and_channel_stable() {
        let credentials = Arc::new(SessionCredentialStore::default());
        let runtime =
            ProviderRuntime::new(None, credentials, "koru-beta").expect("runtime settings");
        assert_eq!(runtime.openai_account(), "koru-beta");
        runtime
            .save_openai_secret(SecretString::new("sk-test").unwrap())
            .unwrap();
        runtime.remove_openai_secret().unwrap();
        assert_eq!(ReasoningChoice::Default.as_token(), "default");
    }
}
