// SPDX-License-Identifier: MPL-2.0

//! Which workspaces exist, whose they are, and what they are called.
//!
//! ## Why the surface needs one at all
//!
//! The app's own index put a `profile_id` and a `profile_name` on every hit and
//! every facet. The store's search carries neither, and says why: *a profile is
//! an authorization and query scope over workspaces, resolved through the
//! catalog before a query arrives*. An index that stored one would be storing a
//! fact about who is allowed to look rather than about what is written.
//!
//! So the resolution the store declines to do is done here, from two calls it
//! already answers — `GET /v1/workspaces`, whose entries carry an `owner`, and
//! `GET /v1/profiles`. That is the whole of this file, and it serves five
//! things a `profile_id` on a hit used to do:
//!
//! - listing Store profiles in a single-choice scope menu;
//! - expanding the selected profile into the workspace ids a query carries;
//! - telling the shell which profile a result belongs to when opening it;
//! - telling a filter menu what a profile is called; and
//! - excluding archived and softly deleted workspaces from ordinary global
//!   search, while making live archived workspaces an explicit fallback.
//!
//! ## Lifecycle makes this required
//!
//! A workspace created a moment ago can wait until the next surface refresh to
//! appear in a filter menu. Archive and deletion state cannot fail open: an
//! unscoped index query means every partition it knows. Global search therefore
//! waits for one complete catalog read, refreshes that catalog when the surface
//! resumes, and refuses the query if the read failed.

use std::collections::HashMap;

use koru_store_core::ids::{ProfileId, WorkspaceId};

use super::store::StoreSearch;

#[derive(Clone, Debug, Default)]
pub(super) struct Catalog {
    pub(super) workspaces: Vec<Entry>,
    profiles: HashMap<ProfileId, String>,
    profile_details: HashMap<ProfileId, String>,
    profile_icons: HashMap<ProfileId, String>,
    profile_colors: HashMap<ProfileId, String>,
    ready: bool,
}

#[derive(Clone, Debug)]
pub(super) struct Entry {
    pub(super) id: WorkspaceId,
    pub(super) name: String,
    pub(super) owner: ProfileId,
    pub(super) archived: bool,
    pub(super) deleted: bool,
}

impl Catalog {
    /// Ask the store what it holds.
    ///
    /// **Failure is distinguishable from an empty store.** [`Catalog::ready`]
    /// stays false so global search can fail closed instead of treating an
    /// unknown archive set as no archived workspaces.
    pub(super) fn load(store: &StoreSearch) -> Self {
        // **Including deleted is required even though none are shown.** An
        // unscoped search means every partition the index knows, so the caller
        // needs the complete catalog to build an explicit, lifecycle-safe
        // workspace scope for the selected profile.
        let Ok(workspaces) = store.call(|client| client.workspaces_including_deleted()) else {
            return Self::default();
        };
        let workspaces = workspaces
            .into_iter()
            .map(|w| Entry {
                id: w.id,
                name: w.name,
                owner: w.owner,
                archived: w.archived,
                deleted: w.deleted_at.is_some(),
            })
            .collect();
        let profile_rows = store.call(|client| client.profiles()).unwrap_or_default();
        let profiles = profile_rows
            .iter()
            .map(|profile| (profile.id, profile.name.clone()))
            .collect();
        let profile_details = profile_rows
            .iter()
            .map(|profile| (profile.id, profile.purpose.clone().unwrap_or_default()))
            .collect();
        let profile_icons = profile_rows
            .iter()
            .filter_map(|profile| profile.icon.clone().map(|icon| (profile.id, icon)))
            .collect();
        let profile_colors = profile_rows
            .iter()
            .filter_map(|profile| {
                profile
                    .color
                    .clone()
                    .or_else(|| {
                        profile
                            .avatar
                            .as_deref()
                            .and_then(|avatar| avatar.strip_prefix("color:"))
                            .map(str::trim)
                            .filter(|color| !color.is_empty())
                            .map(str::to_owned)
                    })
                    .map(|color| (profile.id, color))
            })
            .collect();
        Self {
            workspaces,
            profiles,
            profile_details,
            profile_icons,
            profile_colors,
            ready: true,
        }
    }

    /// Whether the complete workspace catalog was read successfully.
    ///
    /// This used to be a convenience for menus, where an empty fallback was
    /// harmless. It now defines which partitions an ordinary global search is
    /// allowed to ask for, so failure must be distinguishable from a genuinely
    /// empty catalog.
    pub(super) fn ready(&self) -> bool {
        self.ready
    }

    /// Whose workspace this is, if the catalog has heard of it.
    ///
    /// The open path needs it: `PaneRequest::Navigate` still carries a
    /// profile, because the shell still opens a workspace *within* one. A hit
    /// no longer says, so the workspace is asked instead.
    pub(super) fn owner_of(&self, workspace: WorkspaceId) -> Option<ProfileId> {
        self.workspaces
            .iter()
            .find(|entry| entry.id == workspace)
            .map(|entry| entry.owner)
    }

    pub(super) fn profile_name(&self, profile: ProfileId) -> Option<&str> {
        self.profiles.get(&profile).map(String::as_str)
    }

    pub(super) fn profile_detail(&self, profile: ProfileId) -> &str {
        self.profile_details
            .get(&profile)
            .map(String::as_str)
            .unwrap_or_default()
    }

    pub(super) fn profile_icon(&self, profile: ProfileId) -> Option<&str> {
        self.profile_icons.get(&profile).map(String::as_str)
    }

    pub(super) fn profile_color(&self, profile: ProfileId) -> Option<&str> {
        self.profile_colors.get(&profile).map(String::as_str)
    }

    /// Every Store profile, plus any otherwise-orphaned workspace owner, in a
    /// stable order.
    ///
    /// **Ordered by name, not by id**, because this is a menu: a list that
    /// reshuffles between openings is one a person cannot learn.
    pub(super) fn profiles(&self) -> Vec<(ProfileId, String)> {
        let mut seen: Vec<(ProfileId, String)> = self
            .profiles
            .iter()
            .map(|(id, name)| (*id, name.clone()))
            .collect();
        for entry in &self.workspaces {
            if seen.iter().any(|(id, _)| *id == entry.owner) {
                continue;
            }
            let name = self
                .profile_name(entry.owner)
                .map(str::to_owned)
                // A workspace whose owner the store did not list is not a
                // reason to hide the workspace. Name it after itself.
                .unwrap_or_else(|| entry.name.clone());
            seen.push((entry.owner, name));
        }
        seen.sort_by(|a, b| a.1.to_lowercase().cmp(&b.1.to_lowercase()));
        seen
    }

    /// Build one without a store, for a test that needs the resolution but not
    /// the two HTTP calls behind it.
    #[cfg(test)]
    pub(super) fn for_test(
        workspaces: Vec<(WorkspaceId, String, ProfileId)>,
        profiles: Vec<(ProfileId, String)>,
    ) -> Self {
        Self {
            workspaces: workspaces
                .into_iter()
                .map(|(id, name, owner)| Entry {
                    id,
                    name,
                    owner,
                    archived: false,
                    deleted: false,
                })
                .collect(),
            profiles: profiles.into_iter().collect(),
            profile_details: HashMap::new(),
            profile_icons: HashMap::new(),
            profile_colors: HashMap::new(),
            ready: true,
        }
    }

    #[cfg(test)]
    pub(super) fn for_test_with_lifecycle(
        workspaces: Vec<(WorkspaceId, String, ProfileId, bool, bool)>,
        profiles: Vec<(ProfileId, String)>,
    ) -> Self {
        Self {
            workspaces: workspaces
                .into_iter()
                .map(|(id, name, owner, archived, deleted)| Entry {
                    id,
                    name,
                    owner,
                    archived,
                    deleted,
                })
                .collect(),
            profiles: profiles.into_iter().collect(),
            profile_details: HashMap::new(),
            profile_icons: HashMap::new(),
            profile_colors: HashMap::new(),
            ready: true,
        }
    }

    /// Active workspaces the selected profile is allowed to search normally.
    pub(super) fn active_workspaces_of(&self, profile: ProfileId) -> Vec<WorkspaceId> {
        self.workspaces
            .iter()
            .filter(|entry| entry.owner == profile && !entry.archived && !entry.deleted)
            .map(|entry| entry.id)
            .collect()
    }

    /// Live archived workspaces owned by the selected profile.
    pub(super) fn archived_workspaces_of(&self, profile: ProfileId) -> Vec<WorkspaceId> {
        self.workspaces
            .iter()
            .filter(|entry| entry.owner == profile && entry.archived && !entry.deleted)
            .map(|entry| entry.id)
            .collect()
    }

    pub(super) fn has_profile(&self, profile: ProfileId) -> bool {
        self.profiles().iter().any(|(id, _)| *id == profile)
    }

    /// Whether a workspace may appear in ordinary search and its filters.
    ///
    /// Missing is inactive too. Once the catalog is ready it is the authority
    /// for lifecycle; a facet for an unknown partition must fail closed rather
    /// than putting an unclassified workspace in the menu.
    pub(super) fn is_active(&self, workspace: WorkspaceId) -> bool {
        self.workspaces
            .iter()
            .find(|entry| entry.id == workspace)
            .is_some_and(|entry| !entry.archived && !entry.deleted)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn catalog() -> (Catalog, ProfileId, ProfileId, WorkspaceId, WorkspaceId) {
        let work = ProfileId::new();
        let personal = ProfileId::new();
        let acme = WorkspaceId::new();
        let notes = WorkspaceId::new();
        let catalog = Catalog {
            workspaces: vec![
                Entry {
                    id: acme,
                    name: "Acme".into(),
                    owner: work,
                    archived: false,
                    deleted: false,
                },
                Entry {
                    id: notes,
                    name: "Notes".into(),
                    owner: personal,
                    archived: false,
                    deleted: false,
                },
            ],
            profiles: HashMap::from([(work, "Work".to_string()), (personal, "Zed".to_string())]),
            profile_details: HashMap::new(),
            profile_icons: HashMap::new(),
            profile_colors: HashMap::new(),
            ready: true,
        };
        (catalog, work, personal, acme, notes)
    }

    /// **The resolution the store's search declines to do.** Selecting a
    /// profile selects its active workspaces, and this is where that
    /// authorization scope becomes concrete.
    #[test]
    fn a_profile_resolves_to_the_workspaces_it_owns() {
        let (catalog, work, personal, acme, notes) = catalog();
        assert_eq!(catalog.active_workspaces_of(work), vec![acme]);
        assert_eq!(catalog.active_workspaces_of(personal), vec![notes]);
    }

    /// The open path lost the profile that used to ride on every hit, and gets
    /// it from the workspace instead.
    #[test]
    fn a_workspace_says_whose_it_is() {
        let (catalog, work, _, acme, _) = catalog();
        assert_eq!(catalog.owner_of(acme), Some(work));
        assert_eq!(catalog.owner_of(WorkspaceId::new()), None);
    }

    /// A menu that reshuffles between openings is one nobody can learn.
    #[test]
    fn profiles_are_listed_by_name_rather_than_by_id() {
        let (catalog, _, _, _, _) = catalog();
        let names: Vec<String> = catalog.profiles().into_iter().map(|(_, n)| n).collect();
        assert_eq!(names, ["Work", "Zed"]);
    }

    #[test]
    fn a_store_profile_without_workspaces_is_still_selectable() {
        let empty = ProfileId::new();
        let catalog = Catalog::for_test(Vec::new(), vec![(empty, "Empty".into())]);

        assert_eq!(catalog.profiles(), vec![(empty, "Empty".into())]);
        assert!(catalog.has_profile(empty));
        assert!(catalog.active_workspaces_of(empty).is_empty());
    }

    /// **A workspace whose owner the store did not list still appears.** The
    /// filter menu is a convenience; hiding somebody's workspace because a
    /// second call came back short would not be.
    #[test]
    fn a_workspace_with_an_unknown_owner_is_still_listed() {
        let orphan = ProfileId::new();
        let catalog = Catalog {
            workspaces: vec![Entry {
                id: WorkspaceId::new(),
                name: "Salvage".into(),
                owner: orphan,
                archived: false,
                deleted: false,
            }],
            profiles: HashMap::new(),
            profile_details: HashMap::new(),
            profile_icons: HashMap::new(),
            profile_colors: HashMap::new(),
            ready: true,
        };
        assert_eq!(catalog.profiles(), vec![(orphan, "Salvage".to_string())]);
    }

    #[test]
    fn archive_and_delete_sets_are_distinct() {
        let owner = ProfileId::new();
        let active = WorkspaceId::new();
        let archived = WorkspaceId::new();
        let deleted = WorkspaceId::new();
        let archived_deleted = WorkspaceId::new();
        let catalog = Catalog::for_test_with_lifecycle(
            vec![
                (active, "Active".into(), owner, false, false),
                (archived, "Archived".into(), owner, true, false),
                (deleted, "Deleted".into(), owner, false, true),
                (archived_deleted, "Both".into(), owner, true, true),
            ],
            vec![],
        );

        assert_eq!(catalog.active_workspaces_of(owner), vec![active]);
        assert_eq!(catalog.archived_workspaces_of(owner), vec![archived]);
        assert!(catalog.is_active(active));
        assert!(!catalog.is_active(archived));
        assert!(!catalog.is_active(deleted));
        assert!(!catalog.is_active(archived_deleted));
        assert!(!catalog.is_active(WorkspaceId::new()));
    }
}
