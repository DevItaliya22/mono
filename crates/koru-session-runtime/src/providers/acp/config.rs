// SPDX-License-Identifier: MPL-2.0

use std::ffi::OsString;
use std::path::PathBuf;
use std::sync::Arc;
use std::time::Duration;

use koru_session_protocol::{
    DelegationLaunch, ErrorCode, LaunchSelection, ModelName, ProviderKind,
};
use serde_json::Value;

use super::super::process::TransportWake;
use super::*;
use crate::AttachmentPathResolver;

/// Reviewed provider-specific launch policy. Raw argv, environment, metadata
/// and authentication ids remain below AgentControl and are never Debug.
/// Merely constructing a route does not register or qualify a provider.
#[derive(Clone)]
pub struct AcpRoute {
    pub provider: ProviderKind,
    pub supported_versions: Vec<ProviderVersion>,
    pub arguments: Vec<OsString>,
    pub environment: Vec<(OsString, OsString)>,
    pub session_new_meta: Option<Value>,
    /// Only a reviewed, non-interactive cached-credential method may be used.
    pub cached_auth_method: Option<String>,
    pub authenticate_meta: Option<Value>,
    pub(super) dialect: super::routes::AcpDialect,
    pub child_sessions: ChildSessionPolicy,
}

impl AcpRoute {
    pub fn new(
        provider: ProviderKind,
        supported_versions: Vec<ProviderVersion>,
        arguments: Vec<OsString>,
        child_sessions: ChildSessionPolicy,
    ) -> Self {
        Self {
            provider,
            supported_versions,
            arguments,
            environment: Vec::new(),
            session_new_meta: None,
            cached_auth_method: None,
            authenticate_meta: None,
            dialect: super::routes::AcpDialect::Generic,
            child_sessions,
        }
    }
}

#[derive(Clone)]
pub struct AcpConfig {
    pub(super) route: AcpRoute,
    pub(super) executable: OsString,
    pub(super) working_directory: PathBuf,
    pub(super) executable_version: ProviderVersion,
    pub(super) attachment_paths: Arc<dyn AttachmentPathResolver>,
    pub(super) delegation: Option<DelegationLaunch>,
    /// A manual session's explicit model and effort (protocol v16). The
    /// model takes the same `session/set_config_option` route a delegated
    /// model takes; an effort no route can apply is refused at create.
    pub(super) selection: Option<LaunchSelection>,
    /// Stdio MCP servers the session asks the agent to start (`session/new`
    /// `mcpServers`), beside any delegated endpoint.
    pub(super) mcp_servers: Vec<crate::providers::McpServerLaunch>,
    pub(super) maximum_frame_bytes: usize,
    pub(super) maximum_input_bytes: usize,
    pub(super) event_capacity: usize,
    pub(super) request_timeout: Duration,
    pub(super) cancel_timeout: Duration,
    pub(super) shutdown_timeout: Duration,
    pub(super) wake: Arc<TransportWake>,
}

impl AcpConfig {
    pub fn new(
        mut route: AcpRoute,
        executable: impl Into<OsString>,
        working_directory: impl Into<PathBuf>,
        executable_version: ProviderVersion,
        attachment_paths: Arc<dyn AttachmentPathResolver>,
    ) -> Result<Self, ProviderFailure> {
        if route.is_claude() {
            let meta = route
                .session_new_meta
                .get_or_insert_with(|| serde_json::json!({}));
            super::claude::enforce_child_exclusions(meta)?;
        }
        let working_directory = working_directory.into();
        if !working_directory.is_absolute() {
            return Err(failure(
                ErrorCode::InvalidRequest,
                ProviderStage::Spawn,
                ProviderDiagnostic::InvalidInput,
            ));
        }
        route.enforce_environment();
        let executable = executable.into();
        let (executable, arguments) = route.launch(&executable, false)?;
        route.arguments = arguments;
        let request_timeout = Duration::from_secs(10).max(route.dialect.request_timeout_floor());
        Ok(Self {
            route,
            executable,
            working_directory,
            executable_version,
            attachment_paths,
            delegation: None,
            selection: None,
            mcp_servers: Vec::new(),
            maximum_frame_bytes: super::super::DEFAULT_PROVIDER_FRAME_BYTES,
            maximum_input_bytes: 1024 * 1024,
            event_capacity: 256,
            request_timeout,
            cancel_timeout: Duration::from_secs(3),
            shutdown_timeout: Duration::from_secs(3),
            wake: TransportWake::provider_default(),
        })
    }

    #[must_use]
    pub fn with_delegation(mut self, delegation: Option<DelegationLaunch>) -> Self {
        self.delegation = delegation;
        self
    }

    pub fn with_selection(mut self, selection: Option<LaunchSelection>) -> Self {
        self.selection = selection;
        self
    }

    #[must_use]
    pub fn with_mcp_servers(mut self, servers: Vec<crate::providers::McpServerLaunch>) -> Self {
        self.mcp_servers = servers;
        self
    }

    /// The model this session was asked for, from whichever launch carried
    /// it: a delegated launch's, or a manual creation's explicit selection.
    pub(super) fn requested_model(&self) -> Option<ModelName> {
        self.delegation
            .as_ref()
            .and_then(|launch| launch.model.clone())
            .or_else(|| {
                self.selection
                    .as_ref()
                    .and_then(|selection| selection.model.clone())
            })
    }

    pub fn with_limits(mut self, limits: ProbeLimits) -> Result<Self, ProviderFailure> {
        limits.validate()?;
        // A caller's budget never undercuts what the route needs to start.
        self.request_timeout = limits
            .timeout
            .max(self.route.dialect.request_timeout_floor());
        self.shutdown_timeout = limits.timeout;
        self.maximum_frame_bytes = limits.maximum_output_bytes;
        Ok(self)
    }
}
