// SPDX-License-Identifier: MPL-2.0

//! Pure chooser state. Requests are scoped to provider/folder and a minted ID.
use super::*;
use koru_session_protocol::{IdempotencyKey, ProviderKind, ProviderModels, capability};
const PAGE: usize = 8;
#[derive(Clone, PartialEq, Eq)]
pub(crate) struct Target {
    pub(crate) provider: ProviderKind,
    pub(crate) cwd: std::path::PathBuf,
}
pub(crate) struct ModelPicker {
    pub(crate) target: Option<Target>,
    pub(crate) request: Option<IdempotencyKey>,
    pub(crate) catalog: Option<ProviderModels>,
    pub(crate) selected: Option<koru_delegate_core::ModelName>,
    pub(crate) search: TextArea,
    pub(crate) filtered: Vec<usize>,
    pub(crate) page: usize,
    pub(crate) problem: bool,
    pub(crate) offered: bool,
}
impl ModelPicker {
    pub(crate) fn new(
        engine: &mut TextEngine,
        selected: Option<koru_delegate_core::ModelName>,
    ) -> Self {
        Self {
            target: None,
            request: None,
            catalog: None,
            selected,
            search: TextArea::new(engine, koru_draw::theme::role::body()),
            filtered: Vec::new(),
            page: 0,
            problem: false,
            offered: false,
        }
    }
    pub(crate) fn visible(&self) -> impl Iterator<Item = usize> + '_ {
        self.filtered
            .iter()
            .skip(self.page * PAGE)
            .take(PAGE)
            .copied()
    }
    pub(crate) fn filter(&mut self) {
        let query = self.search.value().to_lowercase();
        self.filtered = self
            .catalog
            .as_ref()
            .map(|catalog| {
                catalog
                    .models()
                    .iter()
                    .enumerate()
                    .filter(|(_, model)| model.id.as_str().to_lowercase().contains(query.trim()))
                    .map(|(index, _)| index)
                    .collect()
            })
            .unwrap_or_default();
        self.page = 0;
    }
    pub(crate) fn selection_valid(&self) -> bool {
        self.selected.as_ref().is_none_or(|selected| {
            self.offered
                && self.request.is_none()
                && self.catalog.as_ref().is_some_and(|catalog| {
                    catalog
                        .models()
                        .iter()
                        .any(|m| m.id.as_str() == selected.as_str())
                })
        })
    }
    pub(crate) fn previous_page(&mut self) {
        self.page = self.page.saturating_sub(1);
    }
    pub(crate) fn next_page(&mut self) {
        if (self.page + 1) * PAGE < self.filtered.len() {
            self.page += 1;
        }
    }
    pub(crate) fn controls(&self) -> Vec<Hit> {
        if !self.offered {
            return Vec::new();
        }
        let mut controls = vec![Hit::ModelDefault, Hit::ModelSearch];
        controls.extend(self.visible().map(Hit::Model));
        if self.page > 0 {
            controls.push(Hit::ModelPrevious);
        }
        if (self.page + 1) * PAGE < self.filtered.len() {
            controls.push(Hit::ModelNext);
        }
        if self.request.is_none() {
            controls.push(Hit::ModelRefresh);
        }
        controls
    }
}
impl Delegation {
    pub(crate) fn model_discovery(&mut self) -> Option<(IdempotencyKey, Target)> {
        if !matches!(self.phase, Phase::Editing { .. }) {
            return None;
        }
        let row = self.agent.and_then(|index| self.world.agents.get(index));
        self.models.offered = row.is_some_and(|row| {
            row.readiness
                .capabilities
                .supports(capability::PROVIDER_MODELS)
        });
        if !self.models.offered {
            self.models.offered = self.models.selected.is_some();
            self.models.problem = self.models.selected.is_some();
            self.models.request = None;
            self.models.target = None;
            self.models.catalog = None;
            self.models.filtered.clear();
            self.models.page = 0;
            // An explicit re-run selection remains visible as unavailable until
            // the provider becomes ready, never silently becomes default.
            return None;
        }
        let row = row?;
        let scope = self.chosen_scope()?;
        let cwd = if matches!(scope.scope, WorkspaceScope::Temporary) {
            std::path::PathBuf::from(&self.world.temporary_base)
        } else {
            std::path::PathBuf::from(scope.path.as_ref()?)
        };
        let target = Target {
            provider: row.provider.clone(),
            cwd,
        };
        if self.models.target.as_ref() == Some(&target) {
            return None;
        }
        if self
            .models
            .target
            .as_ref()
            .is_some_and(|old| old.provider != target.provider)
        {
            self.models.selected = None;
        }
        let request = IdempotencyKey::mint();
        self.models.target = Some(target.clone());
        self.models.request = Some(request);
        self.models.catalog = None;
        self.models.filtered.clear();
        self.models.problem = false;
        Some((request, target))
    }
    pub(crate) fn accept_models(
        &mut self,
        request: IdempotencyKey,
        result: Result<ProviderModels, koru_session_protocol::ErrorCode>,
    ) -> bool {
        if self.models.request != Some(request) || !matches!(self.phase, Phase::Editing { .. }) {
            return false;
        }
        self.models.request = None;
        self.models.problem = result.is_err();
        self.models.catalog = result.ok();
        self.models.filter();
        self.clamp_focus();
        true
    }
}
