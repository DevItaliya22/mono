// SPDX-License-Identifier: MPL-2.0

use std::sync::Mutex;

use koru_session_protocol::{
    AttentionState, CapabilitySet, MAX_PROJECT_NAME_CHARS, SessionMode, SessionProfileKey,
    WorkspaceScope,
};

use super::*;

// -------------------------------------------------------------------- fixtures

fn scope() -> WorkspaceScope {
    WorkspaceScope {
        profile: "aaaaaaaa-aaaa-4aaa-8aaa-aaaaaaaaaaaa".parse().unwrap(),
        workspace: "bbbbbbbb-bbbb-4bbb-8bbb-bbbbbbbbbbbb".parse().unwrap(),
    }
}

fn other_scope() -> WorkspaceScope {
    WorkspaceScope {
        profile: "aaaaaaaa-aaaa-4aaa-8aaa-aaaaaaaaaaaa".parse().unwrap(),
        workspace: "cccccccc-cccc-4ccc-8ccc-cccccccccccc".parse().unwrap(),
    }
}

fn create(id: ProjectId, name: &str) -> CreateProject {
    CreateProject {
        project_id: id,
        name: name.to_owned(),
        root_directory: None,
        workspace_scope: None,
    }
}

fn create_scoped(
    id: ProjectId,
    name: &str,
    workspace_scope: Option<WorkspaceScope>,
) -> CreateProject {
    CreateProject {
        workspace_scope,
        ..create(id, name)
    }
}

fn session(id: SessionId, parent: Option<SessionId>, process: ProcessState) -> SessionSummary {
    SessionSummary {
        id,
        parent_session_id: parent,
        profile: SessionProfileKey::BuiltIn("bash".parse().unwrap()),
        display_name: "shell".into(),
        working_directory: None,
        mode: SessionMode::RawPty,
        process,
        controller: koru_session_protocol::ControllerState::Detached,
        managed: None,
        active_turn_id: None,
        attention: AttentionState::default(),
        pending_attachment_count: 0,
        capabilities: CapabilitySet::default(),
        project_id: None,
        handover: Default::default(),
        model: None,
        origin: Default::default(),
        created_at_ms: 0,
    }
}

fn running(id: SessionId, parent: Option<SessionId>) -> SessionSummary {
    session(id, parent, ProcessState::Running)
}

/// Resolve-and-place as one step, the way the host's creation path does it.
fn place(
    state: &mut ProjectState,
    requested: Option<ProjectId>,
    parent: Option<SessionId>,
    live: &[SessionSummary],
    session_id: SessionId,
) -> Result<Option<ProjectId>, ErrorCode> {
    let resolved = state.resolve_placement(requested, parent, &|| live.to_vec())?;
    if let Some(project) = resolved {
        state.place(session_id, project);
    }
    Ok(resolved)
}

// --------------------------------------------------------------------- parsing

#[test]
fn an_absent_registry_and_an_empty_one_both_restore_to_nothing() {
    assert_eq!(
        parse(b"").unwrap(),
        Restored {
            projects: Vec::new(),
            skipped_rows: 0
        }
    );
    // A staged write interrupted before its rename can leave whitespace; that
    // is a registry nobody has written yet, not damage.
    assert_eq!(parse(b"\n  \n").unwrap().projects, Vec::new());
}

#[test]
fn a_registry_round_trips_with_its_scopes_and_roots_intact() {
    let stored = ProjectSummary {
        id: ProjectId::mint(),
        name: "Koru — 계획".into(),
        root_directory: Some("C:\\work tree".into()),
        workspace_scope: Some(scope()),
        created_at_ms: 17,
    };
    let restored = parse(&encode(std::slice::from_ref(&stored))).unwrap();
    assert_eq!(restored.skipped_rows, 0);
    assert_eq!(restored.projects, vec![stored]);
}

#[test]
fn a_corrupt_registry_is_an_error_rather_than_a_silently_empty_one() {
    // Damage and a newer writer are different answers, because only one of
    // them means "this host must not touch these bytes".
    assert_eq!(parse(b"not json at all"), Err(RestoreError::Malformed));
    assert_eq!(
        parse(br#"{"version":1,"projects":"not a list"}"#),
        Err(RestoreError::Malformed)
    );
    assert_eq!(
        parse(br#"{"version":2,"projects":[]}"#),
        Err(RestoreError::UnknownVersion(2))
    );
}

#[test]
fn one_unreadable_row_is_skipped_rather_than_failing_the_whole_file() {
    let kept = ProjectId::mint();
    let empty_name = ProjectId::mint();
    let bytes = format!(
        r#"{{"version":1,"projects":[
            {{"id":"{kept}","name":"Kept","root_directory":null,"workspace_scope":null,"created_at_ms":1}},
            {{"id":"{kept}","name":"Duplicate id","root_directory":null,"workspace_scope":null,"created_at_ms":2}},
            {{"id":"{empty_name}","name":"","root_directory":null,"workspace_scope":null,"created_at_ms":3}}
        ]}}"#
    );
    let restored = parse(bytes.as_bytes()).unwrap();
    assert_eq!(restored.skipped_rows, 2);
    assert_eq!(restored.projects.len(), 1);
    assert_eq!(restored.projects[0].name, "Kept");
}

#[test]
fn restoring_a_file_gives_back_projects_and_no_placements() {
    let stored = ProjectSummary {
        id: ProjectId::mint(),
        name: "Koru".into(),
        root_directory: Some("C:\\work".into()),
        workspace_scope: Some(scope()),
        created_at_ms: 7,
    };
    let state = ProjectState::restored(
        parse(&encode(std::slice::from_ref(&stored)))
            .unwrap()
            .projects,
    );
    assert_eq!(state.list(), vec![stored]);
    // The project is the durable folder; sessions and their placement are not.
    assert_eq!(state.placement(SessionId::mint()), None);
}

// ------------------------------------------------------- names and lifecycle

#[test]
fn names_are_bounded_unicode_and_duplicates_are_legal() {
    let mut state = ProjectState::default();
    let first = state.create(create(ProjectId::mint(), "계획"), 1).unwrap();
    // The id is the identity, so a second project may carry the same name.
    let second = state.create(create(ProjectId::mint(), "계획"), 2).unwrap();
    assert_ne!(first.project.id, second.project.id);

    for refused in ["", "   ", "line\nbreak", "\u{7}bell"] {
        assert_eq!(
            state.create(create(ProjectId::mint(), refused), 3).err(),
            Some(ErrorCode::InvalidRequest),
            "{refused:?} was accepted"
        );
    }
    let longest = "ê".repeat(MAX_PROJECT_NAME_CHARS);
    assert!(state.create(create(ProjectId::mint(), &longest), 3).is_ok());
    assert_eq!(
        state
            .create(create(ProjectId::mint(), &format!("{longest}ê")), 4)
            .err(),
        Some(ErrorCode::InvalidRequest)
    );
    assert_eq!(state.list().len(), 3);
}

#[test]
fn a_replayed_create_is_the_same_project_and_a_changed_one_is_a_conflict() {
    let mut state = ProjectState::default();
    let id = ProjectId::mint();

    let first = state.create(create(id, "Koru"), 10).unwrap();
    assert!(!first.replayed);
    let replay = state.create(create(id, "Koru"), 99).unwrap();
    assert!(replay.replayed, "a retry was treated as a new project");
    assert_eq!(first.project, replay.project);
    assert_eq!(state.list().len(), 1);

    assert_eq!(
        state.create(create(id, "Something else"), 11).err(),
        Some(ErrorCode::IdempotencyConflict)
    );
}

#[test]
fn renaming_and_setting_a_root_edit_the_current_row_and_nothing_else() {
    let mut state = ProjectState::default();
    let id = ProjectId::mint();
    state
        .create(
            CreateProject {
                project_id: id,
                name: "Koru".into(),
                root_directory: None,
                workspace_scope: Some(scope()),
            },
            1,
        )
        .unwrap();

    let renamed = state.rename(id, "Koru Terminal".into()).unwrap();
    assert_eq!(renamed.name, "Koru Terminal");
    assert_eq!(renamed.workspace_scope, Some(scope()));
    assert_eq!(renamed.created_at_ms, 1);

    let rooted = state.set_root(id, Some("C:\\work".into())).unwrap();
    assert_eq!(rooted.root_directory.as_deref(), Some("C:\\work"));
    assert_eq!(rooted.name, "Koru Terminal");
    // Clearing is how "Set folder…" is undone; a missing root refuses to
    // launch rather than falling back, which is the launch path's rule.
    assert_eq!(state.set_root(id, None).unwrap().root_directory, None);

    assert_eq!(
        state.rename(ProjectId::mint(), "ghost".into()).err(),
        Some(ErrorCode::ProjectNotFound)
    );
    assert_eq!(
        state.set_root(ProjectId::mint(), None).err(),
        Some(ErrorCode::ProjectNotFound)
    );
    assert_eq!(
        state.rename(id, String::new()).err(),
        Some(ErrorCode::InvalidRequest)
    );
}

#[test]
fn deletion_is_refused_while_the_project_has_running_sessions() {
    let mut state = ProjectState::default();
    let id = ProjectId::mint();
    state.create(create(id, "Koru"), 1).unwrap();

    let live = SessionId::mint();
    let mut sessions = vec![running(live, None)];
    place(&mut state, Some(id), None, &sessions, live).unwrap();

    assert_eq!(
        state.delete(id, &sessions).err(),
        Some(ErrorCode::ProjectHasSessions),
        "delete became a process operation in disguise"
    );
    assert_eq!(state.list().len(), 1);

    // A starting session counts as running: the process is on its way up.
    sessions[0].process = ProcessState::Starting;
    assert_eq!(
        state.delete(id, &sessions).err(),
        Some(ErrorCode::ProjectHasSessions)
    );

    // An exited one does not: the project is bookkeeping the person can now
    // clear away, and deleting it ends nothing.
    sessions[0].process = ProcessState::Exited { code: Some(0) };
    assert_eq!(state.delete(id, &sessions).unwrap().id, id);
    assert!(state.list().is_empty());
    assert_eq!(state.placement(live), None);
    assert_eq!(
        state.delete(id, &sessions).err(),
        Some(ErrorCode::ProjectNotFound),
        "a deleted id still answered"
    );
}

// -------------------------------------------------------- placement and tree

#[test]
fn a_child_inherits_its_parents_project_and_cannot_be_placed_elsewhere() {
    let mut state = ProjectState::default();
    let home = ProjectId::mint();
    let other = ProjectId::mint();
    state.create(create(home, "Home"), 1).unwrap();
    state.create(create(other, "Other"), 2).unwrap();

    let parent = SessionId::mint();
    let child = SessionId::mint();
    let mut sessions = vec![running(parent, None)];
    place(&mut state, Some(home), None, &sessions, parent).unwrap();

    // Silence inherits.
    assert_eq!(
        place(&mut state, None, Some(parent), &sessions, child),
        Ok(Some(home))
    );
    sessions.push(running(child, Some(parent)));

    // Saying the same thing is allowed; saying a different one is refused
    // rather than quietly splitting the subtree.
    assert_eq!(
        state.resolve_placement(Some(home), Some(parent), &|| sessions.clone()),
        Ok(Some(home))
    );
    assert_eq!(
        state
            .resolve_placement(Some(other), Some(parent), &|| sessions.clone())
            .err(),
        Some(ErrorCode::ProjectMismatch)
    );
    assert_eq!(
        state
            .resolve_placement(Some(ProjectId::mint()), None, &|| sessions.clone())
            .err(),
        Some(ErrorCode::ProjectNotFound)
    );
}

#[test]
fn an_unplaced_parent_leaves_its_child_unplaced_rather_than_guessing() {
    let mut state = ProjectState::default();
    let parent = SessionId::mint();
    let child = SessionId::mint();
    let sessions = vec![running(parent, None)];
    // Nothing placed the parent: the child joins it in the virtual bucket,
    // which is not a project and is never a persisted destination.
    assert_eq!(
        place(&mut state, None, Some(parent), &sessions, child),
        Ok(None)
    );
    assert_eq!(state.placement(child), None);
}

#[test]
fn moving_a_parent_moves_its_subtree_and_a_child_cannot_move_alone() {
    let mut state = ProjectState::default();
    let home = ProjectId::mint();
    let other = ProjectId::mint();
    state.create(create(home, "Home"), 1).unwrap();
    state.create(create(other, "Other"), 2).unwrap();

    let parent = SessionId::mint();
    let child = SessionId::mint();
    let grandchild = SessionId::mint();
    let sessions = vec![
        running(parent, None),
        running(child, Some(parent)),
        running(grandchild, Some(child)),
    ];

    state.assign(parent, Some(home), &sessions).unwrap();
    for member in [parent, child, grandchild] {
        assert_eq!(state.placement(member), Some(home));
    }

    assert_eq!(
        state.assign(child, Some(other), &sessions).err(),
        Some(ErrorCode::ProjectMismatch),
        "a child left its parent's project"
    );

    state.assign(parent, Some(other), &sessions).unwrap();
    for member in [parent, child, grandchild] {
        assert_eq!(state.placement(member), Some(other));
    }

    // Out of every project: the tree draws these under its virtual bucket.
    state.assign(parent, None, &sessions).unwrap();
    for member in [parent, child, grandchild] {
        assert_eq!(state.placement(member), None);
    }

    assert_eq!(
        state.assign(SessionId::mint(), Some(home), &sessions).err(),
        Some(ErrorCode::SessionNotFound)
    );
    assert_eq!(
        state
            .assign(parent, Some(ProjectId::mint()), &sessions)
            .err(),
        Some(ErrorCode::ProjectNotFound)
    );
}

#[test]
fn a_reaped_parent_leaves_an_ordinary_root_that_can_still_be_moved() {
    let mut state = ProjectState::default();
    let home = ProjectId::mint();
    state.create(create(home, "Home"), 1).unwrap();

    let parent = SessionId::mint();
    let child = SessionId::mint();
    // The parent is gone from the host's list; the child is a root now, and
    // refusing to move it would strand it forever.
    let sessions = vec![running(child, Some(parent))];
    state.assign(child, Some(home), &sessions).unwrap();
    assert_eq!(state.placement(child), Some(home));
}

#[test]
fn placement_is_forgotten_when_the_host_forgets_the_session() {
    let mut state = ProjectState::default();
    let id = ProjectId::mint();
    state.create(create(id, "Koru"), 1).unwrap();

    let gone = SessionId::mint();
    let alive = SessionId::mint();
    let both = vec![running(gone, None), running(alive, None)];
    state.assign(gone, Some(id), &both).unwrap();
    state.assign(alive, Some(id), &both).unwrap();

    let remaining = vec![running(alive, None)];
    state.assign(alive, Some(id), &remaining).unwrap();
    assert_eq!(state.placement(gone), None);
    assert_eq!(state.placement(alive), Some(id));
}

// ------------------------------------------------------- the workspace join

/// **The join a workspace's removal walks, and the four populations it has to
/// tell apart.** A session carries a project, a project carries an optional
/// workspace scope, and nothing carries the workspace — so this is the only
/// place the two ends meet, and getting it wrong closes somebody else's
/// terminal.
#[test]
fn a_workspace_owns_only_the_sessions_its_own_scoped_projects_hold() {
    let mut state = ProjectState::default();
    let mine = ProjectId::mint();
    let neighbour = ProjectId::mint();
    let everywhere = ProjectId::mint();
    state
        .create(create_scoped(mine, "Mine", Some(scope())), 0)
        .unwrap();
    state
        .create(
            create_scoped(neighbour, "Neighbour", Some(other_scope())),
            0,
        )
        .unwrap();
    state
        .create(create_scoped(everywhere, "Everywhere", None), 0)
        .unwrap();

    let owned = SessionId::mint();
    let next_door = SessionId::mint();
    let unscoped = SessionId::mint();
    let unplaced = SessionId::mint();
    let live = vec![
        running(owned, None),
        running(next_door, None),
        running(unscoped, None),
        running(unplaced, None),
    ];
    state.place(owned, mine);
    state.place(next_door, neighbour);
    state.place(unscoped, everywhere);

    assert_eq!(state.sessions_in_scope(scope(), &live), vec![owned]);
    assert_eq!(
        state.sessions_in_scope(other_scope(), &live),
        vec![next_door]
    );

    // A workspace this host has never heard of owns nothing. Absent and empty
    // are the same answer to "close its sessions".
    let unknown = WorkspaceScope {
        profile: "aaaaaaaa-aaaa-4aaa-8aaa-aaaaaaaaaaaa".parse().unwrap(),
        workspace: "dddddddd-dddd-4ddd-8ddd-dddddddddddd".parse().unwrap(),
    };
    assert!(state.sessions_in_scope(unknown, &live).is_empty());
}

// ------------------------------------------- following a workspace (SP-15a)

/// The profile a workspace might move to.
fn elsewhere() -> koru_session_protocol::CatalogProfileId {
    "eeeeeeee-eeee-4eee-8eee-eeeeeeeeeeee".parse().unwrap()
}

/// **The repair touches the workspace's projects and nothing else.**
///
/// Not the ones in another workspace that happens to share the owner, and not
/// the unscoped ones — an unscoped project belongs to no workspace and is
/// visible everywhere, so a workspace changing hands says nothing about it.
#[test]
fn rescoping_a_workspace_moves_its_projects_and_leaves_every_other_alone() {
    let mut state = ProjectState::default();
    let mine = ProjectId::mint();
    let neighbour = ProjectId::mint();
    let everywhere = ProjectId::mint();
    state
        .create(create_scoped(mine, "Mine", Some(scope())), 0)
        .unwrap();
    state
        .create(
            create_scoped(neighbour, "Next door", Some(other_scope())),
            1,
        )
        .unwrap();
    state
        .create(create_scoped(everywhere, "Loose", None), 2)
        .unwrap();

    let rescoped = state.rescope_workspace(scope().workspace, elsewhere());

    assert_eq!(rescoped, vec![mine]);
    let scoped = |id: ProjectId| {
        state
            .list()
            .into_iter()
            .find(|project| project.id == id)
            .unwrap()
            .workspace_scope
    };
    assert_eq!(
        scoped(mine),
        Some(WorkspaceScope {
            profile: elsewhere(),
            workspace: scope().workspace,
        }),
        "the workspace half must not move — it is the same workspace"
    );
    assert_eq!(scoped(neighbour), Some(other_scope()));
    assert_eq!(scoped(everywhere), None);
}

/// **Applying it twice is applying it once.** The request names the state to be
/// in rather than the move that happened, so a retry, a repair after a crash
/// and a repair nobody needed are the same call — and the second one reports
/// nothing, because nothing changed.
#[test]
fn rescoping_a_workspace_twice_reports_nothing_the_second_time() {
    let mut state = ProjectState::default();
    let project = ProjectId::mint();
    state
        .create(create_scoped(project, "Mine", Some(scope())), 0)
        .unwrap();

    assert_eq!(
        state.rescope_workspace(scope().workspace, elsewhere()),
        vec![project]
    );
    assert!(
        state
            .rescope_workspace(scope().workspace, elsewhere())
            .is_empty()
    );
}

/// **A stale replay cannot put a project back.** The verb names a destination,
/// so replaying the *first* of two moves after the second landed sends the
/// project back to where it was — which is why the caller is the authority that
/// knows the current owner, and why this test says out loud that the verb
/// itself has no memory to be stale about. The protection is that the request
/// is never a diff: there is no "from" for a replay to disagree with.
#[test]
fn rescoping_reports_only_what_it_changed_over_current_state() {
    let mut state = ProjectState::default();
    let project = ProjectId::mint();
    state
        .create(create_scoped(project, "Mine", Some(scope())), 0)
        .unwrap();
    let third: koru_session_protocol::CatalogProfileId =
        "ffffffff-ffff-4fff-8fff-ffffffffffff".parse().unwrap();

    state.rescope_workspace(scope().workspace, elsewhere());
    assert_eq!(
        state.rescope_workspace(scope().workspace, third),
        vec![project],
        "a second destination moves it again, over whatever it says now"
    );
    // And the first destination is not somewhere it drifts back to on its own.
    assert!(state.rescope_workspace(scope().workspace, third).is_empty());
}

/// A workspace this host has never heard of has no projects to repair, and
/// that is `Ok` with nothing rather than a refusal — for `sessions_in_scope`'s
/// reason: absent and empty are the same answer.
#[test]
fn rescoping_a_workspace_with_no_projects_changes_nothing() {
    let mut state = ProjectState::default();
    state
        .create(create_scoped(ProjectId::mint(), "Loose", None), 0)
        .unwrap();
    assert!(
        state
            .rescope_workspace(other_scope().workspace, elsewhere())
            .is_empty()
    );
}

/// **The orphan area's second resolution, over pure state** (protocol v13).
///
/// One project, the whole typed pair, and the workspace half moves too — which
/// is the difference from `rescope_workspace`, and the reason it needed a verb
/// of its own rather than a second caller of that one.
#[test]
fn setting_a_scope_moves_one_project_and_leaves_its_neighbours_alone() {
    let mut state = ProjectState::default();
    let orphan = ProjectId::mint();
    let neighbour = ProjectId::mint();
    let loose = ProjectId::mint();
    state
        .create(create_scoped(orphan, "Orphan", Some(scope())), 0)
        .unwrap();
    state
        .create(create_scoped(neighbour, "Next door", Some(scope())), 1)
        .unwrap();
    state
        .create(create_scoped(loose, "Loose", None), 2)
        .unwrap();

    let moved = state.set_scope(orphan, Some(other_scope())).unwrap();
    assert!(moved.changed);
    assert_eq!(moved.project.workspace_scope, Some(other_scope()));
    // Its name, root and creation time are not part of the move.
    assert_eq!(moved.project.name, "Orphan");
    assert_eq!(moved.project.created_at_ms, 0);

    let scoped = |id: ProjectId| {
        state
            .list()
            .into_iter()
            .find(|project| project.id == id)
            .unwrap()
            .workspace_scope
    };
    assert_eq!(scoped(orphan), Some(other_scope()));
    assert_eq!(
        scoped(neighbour),
        Some(scope()),
        "a project sharing the old scope was moved along with it"
    );
    assert_eq!(scoped(loose), None);
}

/// **Naming the destination is what makes it idempotent**, and the second call
/// is not merely harmless — it reports that nothing moved, which is what stops
/// the registry above from rewriting a file and invalidating every window over
/// a change nobody made.
#[test]
fn setting_the_same_scope_twice_moves_nothing_the_second_time() {
    let mut state = ProjectState::default();
    let project = ProjectId::mint();
    state
        .create(create_scoped(project, "Orphan", Some(scope())), 0)
        .unwrap();

    assert!(
        state
            .set_scope(project, Some(other_scope()))
            .unwrap()
            .changed
    );
    let again = state.set_scope(project, Some(other_scope())).unwrap();
    assert!(!again.changed, "the destination moved on its second naming");
    assert_eq!(
        again.project.workspace_scope,
        Some(other_scope()),
        "the row is still answered, because 'already there' answers the same question"
    );

    // Clearing is the field's other legal value, not a second verb — the same
    // shape `set_root(None)` has. It is a change once and not twice.
    assert!(state.set_scope(project, None).unwrap().changed);
    assert!(!state.set_scope(project, None).unwrap().changed);
    assert_eq!(
        state
            .set_scope(project, None)
            .unwrap()
            .project
            .workspace_scope,
        None
    );
}

/// A project the registry does not have is `ProjectNotFound`, not a row
/// invented to carry the scope somebody asked for.
#[test]
fn setting_a_scope_on_an_unknown_project_is_refused() {
    let mut state = ProjectState::default();
    state
        .create(create_scoped(ProjectId::mint(), "Real", Some(scope())), 0)
        .unwrap();

    assert_eq!(
        state.set_scope(ProjectId::mint(), Some(scope())).err(),
        Some(ErrorCode::ProjectNotFound)
    );
    assert_eq!(
        state.set_scope(ProjectId::mint(), None).err(),
        Some(ErrorCode::ProjectNotFound)
    );
    assert_eq!(state.list().len(), 1, "a refusal created a project");
}

/// A session that has already exited is not something this call closed, and a
/// session whose subtree was placed as a whole is closed as a whole.
#[test]
fn the_workspace_join_skips_exited_sessions_and_keeps_whole_subtrees() {
    let mut state = ProjectState::default();
    let project = ProjectId::mint();
    state
        .create(create_scoped(project, "Mine", Some(scope())), 0)
        .unwrap();

    let parent = SessionId::mint();
    let child = SessionId::mint();
    let finished = SessionId::mint();
    let live = vec![
        running(parent, None),
        running(child, Some(parent)),
        session(finished, None, ProcessState::Exited { code: Some(0) }),
    ];
    state.place(parent, project);
    state.place(child, project);
    state.place(finished, project);

    let mut affected = state.sessions_in_scope(scope(), &live);
    affected.sort_by_key(|id| id.as_uuid());
    let mut expected = vec![parent, child];
    expected.sort_by_key(|id| id.as_uuid());
    assert_eq!(affected, expected);
    assert!(
        !affected.contains(&finished),
        "a session that had already exited was reported as one this closed"
    );
}

// ------------------------------------------------------------- invalidation

/// Discards drafts. The privacy shape of a project event is asserted in
/// `koru-events`; what matters here is how many were published, and when.
#[derive(Default)]
struct RecordingSink(Mutex<Vec<String>>);

impl SessionLifecycleSink for RecordingSink {
    fn publish(&self, event: koru_events::Draft) {
        let ctx = koru_events::EmitContext::system("koru-session-host-test");
        self.0
            .lock()
            .unwrap_or_else(|held| held.into_inner())
            .push(event.into_envelope(&ctx).name.as_str().to_owned());
    }
}

impl RecordingSink {
    fn published(&self) -> Vec<String> {
        self.0
            .lock()
            .unwrap_or_else(|held| held.into_inner())
            .clone()
    }
}

/// One temp root per test process, named for the process, per the third
/// disk-test exception in `guide/contributing/TESTING.md`. Every test above
/// this line is pure; only the three below reach a filesystem, because
/// staged-write-and-rename is what they are about.
fn process_root(step: &str) -> PathBuf {
    let root = std::env::temp_dir()
        .join(format!("koru-session-projects-{}", std::process::id()))
        .join(step);
    let _ = std::fs::remove_dir_all(&root);
    std::fs::create_dir_all(&root).expect("process-named temp root");
    root
}

#[test]
fn a_registry_write_stages_beside_its_destination_and_replaces_it_whole() {
    let root = process_root("atomic-replace");
    let paths = SessionPaths::from_resolved_directory(root.clone());
    let write = paths.projects_atomic_path();

    write_atomic(&write, b"first").unwrap();
    assert_eq!(std::fs::read(write.destination()).unwrap(), b"first");
    assert!(
        !write.staging().exists(),
        "the staging file outlived its rename"
    );

    // The replacement is whole: the destination is never opened for editing,
    // so a reader sees the previous registry or this one, never a splice.
    write_atomic(&write, b"second and longer").unwrap();
    assert_eq!(
        std::fs::read(write.destination()).unwrap(),
        b"second and longer"
    );
    assert!(!write.staging().exists());

    // A staging file abandoned by a crashed predecessor is reclaimed rather
    // than blocking every later write.
    std::fs::write(write.staging(), b"abandoned").unwrap();
    write_atomic(&write, b"third").unwrap();
    assert_eq!(std::fs::read(write.destination()).unwrap(), b"third");

    let _ = std::fs::remove_dir_all(&root);
}

#[test]
fn projects_survive_a_restart_through_the_bytes_the_host_actually_writes() {
    let root = process_root("restart");
    let paths = SessionPaths::from_resolved_directory(root.clone());
    let sink = Arc::new(RecordingSink::default());
    let id = ProjectId::mint();
    let session_id = SessionId::mint();

    {
        let registry = ProjectRegistry::open(&paths, Arc::clone(&sink) as Arc<_>);
        registry
            .create(
                CreateProject {
                    project_id: id,
                    name: "Koru".into(),
                    root_directory: Some("C:\\work".into()),
                    workspace_scope: Some(scope()),
                },
                7,
            )
            .unwrap();
        // A replay writes nothing and says nothing.
        registry
            .create(
                CreateProject {
                    project_id: id,
                    name: "Koru".into(),
                    root_directory: Some("C:\\work".into()),
                    workspace_scope: Some(scope()),
                },
                8,
            )
            .unwrap();
        let sessions = vec![running(session_id, None)];
        registry
            .assign(session_id, Some(id), &|| sessions.clone())
            .unwrap();
        assert_eq!(
            registry.decorate(sessions)[0].project_id,
            Some(id),
            "a summary left the host without its placement"
        );
    }

    let restarted = ProjectRegistry::open(&paths, Arc::clone(&sink) as Arc<_>);
    let projects = restarted.list();
    assert_eq!(projects.len(), 1);
    assert_eq!(projects[0].id, id);
    assert_eq!(projects[0].root_directory.as_deref(), Some("C:\\work"));
    assert_eq!(projects[0].workspace_scope, Some(scope()));
    // Projects come back; the session and its placement do not.
    assert_eq!(restarted.placement(session_id), None);

    let subscriber = restarted.subscribe(SubscriptionId::new(1).unwrap());
    restarted
        .delete(id, &|| Vec::new())
        .expect("no session is running");
    assert_eq!(subscriber.try_recv(), Ok(CatalogEvent::ProjectsChanged));
    assert!(
        subscriber.try_recv().is_err(),
        "one committed change, one invalidation"
    );
    assert_eq!(
        sink.published(),
        vec!["terminal.project.created", "terminal.project.deleted"],
        "a replayed create or a refused mutation published something"
    );

    // And the deletion is on disk, not only in memory.
    assert!(
        ProjectRegistry::open(&paths, Arc::clone(&sink) as Arc<_>)
            .list()
            .is_empty()
    );

    let _ = std::fs::remove_dir_all(&root);
}

/// **A move is committed before it is announced, and a no-op is neither.**
///
/// The registry's contract for every mutation: write the file, then tell
/// everybody, so nothing is invalidated toward a registry that did not land.
/// The second half is what naming a destination buys — a retry rewrites no file
/// and wakes no window, because there is nothing for either of them to catch up
/// to. `rescope_workspace` takes exactly this position; so does a replayed
/// `create`.
#[test]
fn a_scope_move_is_written_then_announced_and_a_repeat_is_neither() {
    let root = process_root("scope");
    let paths = SessionPaths::from_resolved_directory(root.clone());
    let sink = Arc::new(RecordingSink::default());
    let id = ProjectId::mint();

    let registry = ProjectRegistry::open(&paths, Arc::clone(&sink) as Arc<_>);
    registry
        .create(create_scoped(id, "Orphan", Some(scope())), 7)
        .unwrap();
    let watch = registry.subscribe(SubscriptionId::new(1).unwrap());
    assert!(watch.try_recv().is_err(), "the create predates the watch");

    let moved = registry.set_scope(id, Some(other_scope())).unwrap();
    assert_eq!(moved.workspace_scope, Some(other_scope()));
    assert_eq!(watch.try_recv(), Ok(CatalogEvent::ProjectsChanged));
    assert!(
        watch.try_recv().is_err(),
        "one committed change, one invalidation"
    );

    // The same destination again: no write, no invalidation, same answer.
    let again = registry.set_scope(id, Some(other_scope())).unwrap();
    assert_eq!(again.workspace_scope, Some(other_scope()));
    assert!(
        watch.try_recv().is_err(),
        "a move that changed nothing woke every window"
    );

    // An unknown project neither writes nor announces.
    assert_eq!(
        registry.set_scope(ProjectId::mint(), Some(scope())).err(),
        Some(ErrorCode::ProjectNotFound)
    );
    assert!(watch.try_recv().is_err());

    // And it is on disk, not only in memory: the move survives a restart, with
    // the project's identity, name and creation time intact — which is the
    // whole reason this is not a delete-and-recreate.
    drop(registry);
    let restarted = ProjectRegistry::open(&paths, Arc::clone(&sink) as Arc<_>).list();
    assert_eq!(restarted.len(), 1);
    assert_eq!(restarted[0].id, id);
    assert_eq!(restarted[0].name, "Orphan");
    assert_eq!(restarted[0].created_at_ms, 7);
    assert_eq!(restarted[0].workspace_scope, Some(other_scope()));

    // Scope is not a lifecycle fact, so moving one publishes no event — the
    // same silence `rename` and `set_root` keep.
    assert_eq!(sink.published(), vec!["terminal.project.created"]);

    let _ = std::fs::remove_dir_all(&root);
}

#[test]
fn an_unreadable_registry_is_preserved_before_the_host_starts_empty() {
    let root = process_root("quarantine");
    let paths = SessionPaths::from_resolved_directory(root.clone());
    std::fs::write(paths.projects_file(), b"{ not a registry").unwrap();

    let sink = Arc::new(RecordingSink::default());
    let registry = ProjectRegistry::open(&paths, sink as Arc<_>);
    assert!(registry.list().is_empty());
    assert_eq!(
        std::fs::read(registry.quarantine_path()).unwrap(),
        b"{ not a registry",
        "the unreadable registry was not preserved"
    );
    // Copied, never moved: recovery must not destroy what it was called to
    // preserve, so the original is still exactly where it was.
    assert_eq!(
        std::fs::read(paths.projects_file()).unwrap(),
        b"{ not a registry"
    );

    let _ = std::fs::remove_dir_all(&root);
}
