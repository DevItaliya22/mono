// SPDX-License-Identifier: MPL-2.0

//! OpenCode owns configuration, credentials and catalog resolution. No prompt is sent.
use super::*;
use crate::AttachmentPathResolver;
use koru_session_protocol::{ModelName, ProviderModel, ProviderModels};
use std::{ffi::OsString, path::PathBuf, sync::Arc};

mod accounts;

const DISCOVERY_BUDGET: Duration = Duration::from_secs(25);

pub(super) fn discover(
    route: &AcpRoute,
    executable: OsString,
    cwd: PathBuf,
    attachments: Arc<dyn AttachmentPathResolver>,
) -> Result<ProviderModels, ProviderFailure> {
    match route.dialect {
        routes::AcpDialect::Generic => {
            return Err(failure(
                ErrorCode::CapabilityNotNegotiated,
                ProviderStage::Discovery,
                ProviderDiagnostic::HandshakeRejected,
            ));
        }
        routes::AcpDialect::OpenCode => {}
        _ => return discover_session_options(route, executable, cwd, attachments),
    }
    let started = Instant::now();
    let operation = koru_session_protocol::IdempotencyKey::mint();
    tracing::info!(target: "koru::acp", event = "acp.models.discovery.started", provider = "opencode", operation = %operation);
    let result = (|| {
        if !cwd.is_absolute() || !cwd.is_dir() {
            return Err(invalid_catalog());
        }
        let executable = resolve_probe_executable(&executable)?;
        let probe =
            AcpCompatibilityProbe::new(route.clone(), executable.clone(), cwd.clone(), attachments);
        let deadline = started + DISCOVERY_BUDGET;
        probe.version(ProbeLimits {
            timeout: remaining(deadline)?.min(Duration::from_secs(5)),
            ..ProbeLimits::default()
        })?;
        let connections = if let Some(content) = accounts::environment_content(route)? {
            accounts::parse(content.as_bytes())?
        } else {
            let output = catalog_command(
                route,
                &executable,
                &cwd,
                &["debug", "paths"],
                remaining(deadline)?.min(Duration::from_secs(5)),
            )?;
            accounts::read(accounts::data_path(output.stdout())?)?
        };
        let output = catalog_command(
            route,
            &executable,
            &cwd,
            &["models"],
            remaining(deadline)?.min(Duration::from_secs(20)),
        )?;
        let models = parse_catalog(output.stdout())?;
        tracing::info!(target: "koru::acp", event = "acp.models.connections.filtered", provider = "opencode", operation = %operation,
            resolved_models = models.models().len(), explicit_connections = connections.len());
        remaining(deadline)?;
        accounts::intersect(models, &connections)
    })();
    tracing::info!(target: "koru::acp", event = "acp.models.discovery.finished", provider = "opencode", operation = %operation,
        success = result.is_ok(), count = result.as_ref().map_or(0, |catalog| catalog.models().len()),
        elapsed_ms = started.elapsed().as_millis() as u64);
    result
}

/// The models a route advertises through its session's `model` config
/// option (AGENTUX I3): a discovery session is started exactly as a chat's
/// would be — version gate, `initialize`, `session/new`, the route's own
/// authentication wait — its advertised options are kept, and it is shut
/// down. No prompt is sent. A route that advertises no `model` option
/// answers an empty catalog, which the picker shows as no level.
fn discover_session_options(
    route: &AcpRoute,
    executable: OsString,
    cwd: PathBuf,
    attachments: Arc<dyn AttachmentPathResolver>,
) -> Result<ProviderModels, ProviderFailure> {
    let started = Instant::now();
    let operation = koru_session_protocol::IdempotencyKey::mint();
    tracing::info!(target: "koru::acp", event = "acp.models.discovery.started", provider = %route.provider, operation = %operation);
    let result = (|| {
        if !cwd.is_absolute() || !cwd.is_dir() {
            return Err(invalid_catalog());
        }
        let executable = resolve_probe_executable(&executable)?;
        let probe = AcpCompatibilityProbe::new(route.clone(), executable, cwd, attachments);
        let models = probe.session_models(ProbeLimits {
            timeout: SESSION_DISCOVERY_TIMEOUT,
            ..ProbeLimits::default()
        })?;
        ProviderModels::try_from(models).map_err(|_| invalid_catalog())
    })();
    tracing::info!(target: "koru::acp", event = "acp.models.discovery.finished", provider = %route.provider, operation = %operation,
        success = result.is_ok(), count = result.as_ref().map_or(0, |catalog| catalog.models().len()),
        elapsed_ms = started.elapsed().as_millis() as u64);
    result
}

/// A discovery session waits for the route's own start — Claude's wrapper
/// authentication, Grok's inventory — which the readiness probe's budget
/// has been seen to cut short under load.
const SESSION_DISCOVERY_TIMEOUT: Duration = Duration::from_secs(20);

/// The `model` option's values as a catalog: a flat list under the route's
/// own kind, or ACP's groups under each group's id. A value that is not a
/// model name, or a repeat, is left out; nothing else the option carries
/// (names, descriptions) is kept.
pub(super) fn config_models(session: &Value, fallback_group: &str) -> Vec<ProviderModel> {
    let Ok(option) = option(session) else {
        return Vec::new();
    };
    let Some(entries) = option["options"].as_array() else {
        return Vec::new();
    };
    let mut seen = std::collections::BTreeSet::new();
    let mut models = Vec::new();
    let mut keep = |value: &Value, group: &str| {
        let Some(id) = value["value"]
            .as_str()
            .and_then(|value| value.parse::<ModelName>().ok())
        else {
            return;
        };
        let group: ModelName = match group.parse() {
            Ok(group) => group,
            Err(_) => match fallback_group.parse() {
                Ok(group) => group,
                Err(_) => return,
            },
        };
        if seen.insert(id.clone()) && models.len() < koru_session_protocol::MAX_PROVIDER_MODELS {
            models.push(ProviderModel { id, group });
        }
    };
    for entry in entries {
        match entry["options"].as_array() {
            Some(members) => {
                let group = entry["group"]
                    .as_str()
                    .or_else(|| entry["name"].as_str())
                    .unwrap_or(fallback_group);
                for member in members {
                    keep(member, group);
                }
            }
            None => keep(entry, fallback_group),
        }
    }
    models
}

fn remaining(deadline: Instant) -> Result<Duration, ProviderFailure> {
    deadline
        .checked_duration_since(Instant::now())
        .filter(|duration| !duration.is_zero())
        .ok_or_else(|| {
            failure(
                ErrorCode::ProviderUnavailable,
                ProviderStage::Discovery,
                ProviderDiagnostic::Timeout,
            )
        })
}

fn catalog_command(
    route: &AcpRoute,
    executable: &std::ffi::OsStr,
    cwd: &std::path::Path,
    tokens: &[&'static str],
    timeout: Duration,
) -> Result<super::super::discovery::ProbeOutput, ProviderFailure> {
    // Only fixed adapter-owned tokens reach Windows batch launchers.
    let (program, mut args) = route.launch(executable, true)?;
    args.pop().ok_or_else(invalid_catalog)?;
    args.extend(tokens.iter().map(|token| OsString::from(*token)));
    let command = args
        .iter()
        .fold(ProbeCommand::new(program), |command, arg| command.arg(arg))
        .current_dir(cwd.to_path_buf());
    let command = route
        .environment
        .iter()
        .fold(command, |command, (key, value)| command.env(key, value));
    let output = run_probe_command(
        command,
        ProbeLimits {
            timeout,
            maximum_output_bytes: 1024 * 1024,
        },
    )?;
    if !output.success() {
        return Err(invalid_catalog());
    }
    Ok(output)
}

fn invalid_catalog() -> ProviderFailure {
    failure(
        ErrorCode::ProviderIncompatible,
        ProviderStage::Initialize,
        ProviderDiagnostic::HandshakeRejected,
    )
}

fn parse_catalog(bytes: &[u8]) -> Result<ProviderModels, ProviderFailure> {
    let text = std::str::from_utf8(bytes).map_err(|_| invalid_catalog())?;
    let mut models = std::collections::BTreeMap::new();
    for line in text.lines().filter(|line| !line.is_empty()) {
        let id: ModelName = line.parse().map_err(|_| invalid_catalog())?;
        let (group, name) = line.split_once('/').ok_or_else(invalid_catalog)?;
        if name.is_empty() {
            return Err(invalid_catalog());
        }
        let group = group.parse().map_err(|_| invalid_catalog())?;
        models.insert(id.clone(), ProviderModel { id, group });
        if models.len() > koru_session_protocol::MAX_PROVIDER_MODELS {
            return Err(invalid_catalog());
        }
    }
    models
        .into_values()
        .collect::<Vec<_>>()
        .try_into()
        .map_err(|_| invalid_catalog())
}

pub(super) fn selection_unavailable() -> ProviderFailure {
    failure(
        ErrorCode::ProviderModelUnavailable,
        ProviderStage::Initialize,
        ProviderDiagnostic::HandshakeRejected,
    )
}

fn option(response: &Value) -> Result<&Value, ProviderFailure> {
    let options = response
        .get("configOptions")
        .and_then(Value::as_array)
        .ok_or_else(selection_unavailable)?;
    let mut matching = options.iter().filter(|option| option["id"] == "model");
    let option = matching.next().ok_or_else(selection_unavailable)?;
    if matching.next().is_some() || option["type"] != "select" {
        return Err(selection_unavailable());
    }
    Ok(option)
}

pub(super) fn validate_selection(
    response: &Value,
    selected: &ModelName,
) -> Result<(), ProviderFailure> {
    let options = option(response)?["options"]
        .as_array()
        .ok_or_else(selection_unavailable)?;
    // ACP accepts either ungrouped options or groups containing options.
    let found = options.iter().any(|option| {
        option["value"].as_str() == Some(selected.as_str())
            || option["options"].as_array().is_some_and(|group| {
                group
                    .iter()
                    .any(|item| item["value"].as_str() == Some(selected.as_str()))
            })
    });
    if found {
        Ok(())
    } else {
        Err(selection_unavailable())
    }
}

pub(super) fn validate_applied(
    response: &Value,
    selected: &ModelName,
) -> Result<(), ProviderFailure> {
    if option(response)?["currentValue"].as_str() == Some(selected.as_str()) {
        Ok(())
    } else {
        Err(selection_unavailable())
    }
}

#[cfg(test)]
pub(super) mod tests;
