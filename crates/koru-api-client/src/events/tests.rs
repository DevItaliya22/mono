// SPDX-License-Identifier: MPL-2.0

//! The catalog helper's own rules, with no gateway in sight.
//!
//! What needs a running gateway — a mutation through one client arriving as an
//! invalidation in a second, with no polling — is
//! `apps/koru-agent/tests/gateway_catalog_events.rs`, because this crate must not
//! open a store to test a string comparison and two atomics.

use super::*;

/// Every row `koru-store-api` publishes after a catalog commit invalidates
/// exactly one half, and the half is the one a menu would refetch.
#[test]
fn every_committed_catalog_fact_invalidates_one_half() {
    for name in [
        "profile.created",
        "profile.renamed",
        "profile.appearance_changed",
    ] {
        assert_eq!(
            catalog_change(name),
            Some(CatalogChange::Profiles),
            "{name}"
        );
    }
    for name in [
        "workspace.catalog.created",
        "workspace.catalog.renamed",
        "workspace.catalog.appearance_changed",
        "workspace.catalog.archived",
        "workspace.catalog.unarchived",
        "workspace.catalog.deleted",
        "workspace.catalog.restored",
        "workspace.catalog.imported",
        "workspace.catalog.reordered",
        // The app's own rows and the worker's derivation ride the same prefix,
        // so a workspace created by koru.exe invalidates a Terminal window's
        // menu without a second classification rule.
        "workspace.created",
        "workspace.observed",
        "workspace.deleted",
    ] {
        assert_eq!(
            catalog_change(name),
            Some(CatalogChange::Workspaces),
            "{name}"
        );
    }
}

/// And the rest of a busy bus is not a menu refresh. `document.*` is the one
/// that matters: a person typing produces those continuously, and a banner that
/// refetched on each would be a poller with extra steps.
#[test]
fn ordinary_traffic_invalidates_nothing() {
    for name in [
        "document.created",
        "document.updated",
        "document.trashed",
        "search.opened",
        "import.completed",
        "app.session.started",
        "voice.dictation.completed",
        "terminal.session.started",
        "terminal.project.created",
    ] {
        assert_eq!(catalog_change(name), None, "{name}");
    }
}

/// The prefixes are checked against the registry rather than a list written
/// here, so a row added later that nothing invalidates on is caught by the
/// packet that adds it and not by a person reading a menu that never refreshes.
#[test]
fn the_registry_agrees_about_which_rows_are_catalog_rows() {
    for spec in koru_events::REGISTRY {
        let expected = match spec.name.split('.').next() {
            Some("profile") => Some(CatalogChange::Profiles),
            Some("workspace") => Some(CatalogChange::Workspaces),
            _ => None,
        };
        assert_eq!(catalog_change(spec.name), expected, "{}", spec.name);
    }
}

/// Sticky until taken, cleared by taking, and the two halves are independent —
/// which is what makes "fetch on open" exactly one refetch per batch of facts.
#[test]
fn a_flag_survives_a_closed_surface_and_clears_on_the_fetch() {
    let flags = CatalogFlags::default();
    assert!(!flags.is_stale(CatalogChange::Workspaces));
    assert!(!flags.take(CatalogChange::Workspaces));

    // Three facts arrive while the menu is shut.
    flags.invalidate(CatalogChange::Workspaces);
    flags.invalidate(CatalogChange::Workspaces);
    flags.invalidate(CatalogChange::Workspaces);
    assert!(flags.is_stale(CatalogChange::Workspaces));
    assert!(
        !flags.is_stale(CatalogChange::Profiles),
        "a workspace fact is not a reason to refetch profiles"
    );

    // The menu opens: one refetch.
    assert!(flags.take(CatalogChange::Workspaces));
    // It opens again with nothing in between: none.
    assert!(!flags.take(CatalogChange::Workspaces));
}

/// The classification runs on the *envelope's* name, so a page read back off
/// the journal invalidates the same halves a live frame does.
#[test]
fn an_envelope_classifies_the_same_way_its_name_does() {
    let envelope = koru_events::Draft::from(koru_events::WorkspaceCatalogArchived {
        workspace_id: koru_store_core::ids::WorkspaceId::new().as_uuid(),
    })
    .into_envelope(&koru_events::EmitContext::system("store"));
    assert_eq!(
        catalog_change(envelope.name.as_str()),
        Some(CatalogChange::Workspaces)
    );
}
