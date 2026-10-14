// SPDX-License-Identifier: MPL-2.0

//! The session host's project registry.
//!
//! Projects are the one thing in this domain that outlives the process, so
//! this module owns a file; sessions do not, and their placement lives only
//! in memory beside it. Every mutation is a *typed operation applied to the
//! current state under one lock* — no caller ever hands the host a whole
//! registry it read a moment ago, so two windows editing at once cannot make
//! one of them win by being slower.
//!
//! The model and its storage are deliberately two types. [`ProjectState`]
//! decides — inheritance, refusals, subtrees — and knows nothing about files,
//! clocks, subscribers or events; [`ProjectRegistry`] is that state plus the
//! atomic write, the invalidation stream and the two registry events. That
//! split is why almost every test below this module touches no disk.
//!
//! Lock order, stated because it is the only one: this registry's mutex is
//! taken before the session registry is read, never after. Nothing in
//! `koru-session-runtime` knows projects exist, so the reverse order does not
//! occur.

use std::collections::{BTreeMap, BTreeSet, VecDeque};
use std::io::Write as _;
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex, MutexGuard, mpsc};

use koru_events::{
    TerminalProjectCreated, TerminalProjectDeleted, TerminalProjectRoot, TerminalProjectScope,
};
use koru_session_paths::{AtomicPath, SessionPaths};
use koru_session_protocol::{
    CatalogEvent, CatalogProfileId, CatalogWorkspaceId, ErrorCode, ProcessState, ProjectId,
    ProjectSummary, SessionId, SessionSummary, WorkspaceScope,
};
use koru_session_runtime::SessionLifecycleSink;
use koru_session_wire::{CreateProject, SubscriptionId};

/// The persisted shape's version.
///
/// Read strictly: a file from a newer host is quarantined rather than
/// reinterpreted, because guessing at a shape we do not know is how a
/// downgrade silently drops somebody's projects.
pub const FILE_VERSION: u32 = 1;

/// One notification is enough, because every notification says the same
/// thing. A subscriber that has not drained the one already queued will
/// re-read the catalogue anyway, so dropping the second loses nothing — and
/// that is what makes this stream bounded without a policy.
const CATALOG_QUEUE: usize = 1;

#[derive(serde::Serialize, serde::Deserialize)]
struct RegistryFile {
    version: u32,
    projects: Vec<ProjectSummary>,
}

/// What a parse recovered, and what it could not.
///
/// A single unreadable row is skipped rather than failing the whole file: one
/// damaged record must not be a refusal to open a registry that is otherwise
/// intact. The count is reported so the host can say so out loud.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Restored {
    pub projects: Vec<ProjectSummary>,
    pub skipped_rows: usize,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum RestoreError {
    /// Not the shape this host writes. The bytes are kept, never rewritten
    /// in place.
    Malformed,
    /// A file version this host does not understand.
    UnknownVersion(u32),
}

impl std::fmt::Display for RestoreError {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Malformed => formatter.write_str("the project registry could not be decoded"),
            Self::UnknownVersion(version) => {
                write!(formatter, "unknown project registry version {version}")
            }
        }
    }
}

/// Decode a registry file. Pure: no path, no clock, no filesystem — which is
/// what makes restart and corruption behavior testable without a disk.
pub fn parse(bytes: &[u8]) -> Result<Restored, RestoreError> {
    // An empty or whitespace-only file is a registry nobody has written yet,
    // not damage: the staged-write path can leave one behind after a crash.
    if bytes.iter().all(u8::is_ascii_whitespace) {
        return Ok(Restored {
            projects: Vec::new(),
            skipped_rows: 0,
        });
    }
    let file: RegistryFile = serde_json::from_slice(bytes).map_err(|_| RestoreError::Malformed)?;
    if file.version != FILE_VERSION {
        return Err(RestoreError::UnknownVersion(file.version));
    }
    let mut projects = Vec::with_capacity(file.projects.len());
    let mut seen = BTreeSet::new();
    let mut skipped_rows = 0;
    for project in file.projects {
        // A row is kept only if it would have been accepted as a mutation.
        // Anything else is somebody else's writer, or damage.
        if project.validate().is_err() || !seen.insert(project.id) {
            skipped_rows += 1;
            continue;
        }
        projects.push(project);
    }
    Ok(Restored {
        projects,
        skipped_rows,
    })
}

/// Serialize the whole registry. There is no partial write: a registry is
/// small, and a complete replacement is what makes the rename below atomic.
pub fn encode(projects: &[ProjectSummary]) -> Vec<u8> {
    let file = RegistryFile {
        version: FILE_VERSION,
        projects: projects.to_vec(),
    };
    let mut bytes = serde_json::to_vec_pretty(&file).unwrap_or_default();
    bytes.push(b'\n');
    bytes
}

/// Write the complete registry beside its destination and replace it in one
/// filesystem operation. The destination is never edited in place, so a
/// reader either sees the previous registry or this one.
pub fn write_atomic(path: &AtomicPath, bytes: &[u8]) -> std::io::Result<()> {
    if let Some(parent) = path.destination().parent() {
        std::fs::create_dir_all(parent)?;
    }
    let result = (|| {
        // A staging file left by a crashed predecessor is ours to reclaim: it
        // was never published, so nothing can be reading it.
        let mut file = std::fs::File::create(path.staging())?;
        file.write_all(bytes)?;
        file.sync_all()?;
        drop(file);
        std::fs::rename(path.staging(), path.destination())
    })();
    if result.is_err() {
        let _ = std::fs::remove_file(path.staging());
    }
    result
}

/// What a committed create was: a new project, or the same one again.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Created {
    pub project: ProjectSummary,
    /// True when this frame had already been applied. A replay changes
    /// nothing, so it writes nothing and says nothing.
    pub replayed: bool,
}

/// What a committed scope rewrite was: a move, or the destination again.
///
/// [`Created`]'s shape, for [`Created`]'s reason. A verb that names where
/// something should be has no way to fail on a second attempt, so the useful
/// fact is not whether it succeeded but whether anything moved — which is what
/// decides if a file is rewritten and every window is told to re-read.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Moved {
    pub project: ProjectSummary,
    /// False when the project already carried this scope.
    pub changed: bool,
}

/// The project model: decisions over current state, and nothing else.
#[derive(Debug, Default)]
pub struct ProjectState {
    projects: BTreeMap<ProjectId, ProjectSummary>,
    /// Placement dies with the session, exactly as the session does. Nothing
    /// here is persisted: a restarted Agent has projects and no sessions.
    placements: BTreeMap<SessionId, ProjectId>,
}

impl ProjectState {
    pub fn restored(projects: Vec<ProjectSummary>) -> Self {
        Self {
            projects: projects
                .into_iter()
                .map(|project| (project.id, project))
                .collect(),
            placements: BTreeMap::new(),
        }
    }

    /// A stable listing order that is deliberately not a semantic one: there
    /// is no `sort_key`, and the surface sorts by name under its own
    /// collation. Oldest first, id breaking a tie, so two readers agree.
    pub fn list(&self) -> Vec<ProjectSummary> {
        let mut rows: Vec<_> = self.projects.values().cloned().collect();
        rows.sort_by(|left, right| {
            left.created_at_ms
                .cmp(&right.created_at_ms)
                .then_with(|| left.id.as_uuid().cmp(&right.id.as_uuid()))
        });
        rows
    }

    /// Create a project under the client's own id.
    ///
    /// The id *is* the retry key: the same frame twice is the same project,
    /// and the same id carrying different content is a conflict rather than a
    /// silent overwrite of somebody else's row.
    pub fn create(
        &mut self,
        request: CreateProject,
        created_at_ms: i64,
    ) -> Result<Created, ErrorCode> {
        let candidate = ProjectSummary {
            id: request.project_id,
            name: request.name,
            root_directory: request.root_directory,
            workspace_scope: request.workspace_scope,
            created_at_ms,
        };
        candidate.validate()?;
        if let Some(existing) = self.projects.get(&candidate.id) {
            let replay = existing.name == candidate.name
                && existing.root_directory == candidate.root_directory
                && existing.workspace_scope == candidate.workspace_scope;
            return replay
                .then(|| Created {
                    project: existing.clone(),
                    replayed: true,
                })
                .ok_or(ErrorCode::IdempotencyConflict);
        }
        self.projects.insert(candidate.id, candidate.clone());
        Ok(Created {
            project: candidate,
            replayed: false,
        })
    }

    pub fn rename(&mut self, id: ProjectId, name: String) -> Result<ProjectSummary, ErrorCode> {
        koru_session_protocol::validate_project_name(&name)?;
        self.edit(id, |project| project.name = name)
    }

    pub fn set_root(
        &mut self,
        id: ProjectId,
        root_directory: Option<String>,
    ) -> Result<ProjectSummary, ErrorCode> {
        koru_session_protocol::validate_project_root(root_directory.as_deref())?;
        self.edit(id, |project| project.root_directory = root_directory)
    }

    /// **Give one project the scope it should now carry** (protocol v13).
    ///
    /// The orphan area's second resolution. One typed operation over the
    /// current row, exactly like [`Self::rename`] and [`Self::set_root`] — the
    /// caller hands a destination, never a registry it read a moment ago, so
    /// two windows moving the same project cannot make the slower one win.
    ///
    /// **A statement of the state to be in, not a diff**, which is the whole
    /// of its idempotence: the same pair twice leaves the same row, and a
    /// replay after a crash lands on the destination rather than re-applying a
    /// move that already happened.
    ///
    /// This host asks nothing about the workspace. Whether a destination is
    /// live, archived or absent is a *catalog* question, and the session host
    /// has no catalog: derived visibility is the surface's rule (ADR-264), and
    /// a host that tried to enforce it would be a second authority on Koru's
    /// workspaces with no way to read them. The surface refuses an archived or
    /// deleted destination where it can see one; the registry stores what it
    /// is told, which is the same posture `create` already takes.
    pub fn set_scope(
        &mut self,
        id: ProjectId,
        workspace_scope: Option<WorkspaceScope>,
    ) -> Result<Moved, ErrorCode> {
        let project = self
            .projects
            .get_mut(&id)
            .ok_or(ErrorCode::ProjectNotFound)?;
        let changed = project.workspace_scope != workspace_scope;
        project.workspace_scope = workspace_scope;
        Ok(Moved {
            project: project.clone(),
            changed,
        })
    }

    /// Delete a project. Bookkeeping only: it ends no process, which is why
    /// it is refused rather than cascading while sessions are still running.
    pub fn delete(
        &mut self,
        id: ProjectId,
        live: &[SessionSummary],
    ) -> Result<ProjectSummary, ErrorCode> {
        if !self.projects.contains_key(&id) {
            return Err(ErrorCode::ProjectNotFound);
        }
        self.prune(live);
        let occupied = live.iter().any(|session| {
            matches!(
                session.process,
                ProcessState::Starting | ProcessState::Running
            ) && self.placements.get(&session.id) == Some(&id)
        });
        if occupied {
            return Err(ErrorCode::ProjectHasSessions);
        }
        let removed = self
            .projects
            .remove(&id)
            .ok_or(ErrorCode::ProjectNotFound)?;
        // An ended session left behind in this project loses its placement
        // with the project, so a stale row cannot resurrect a deleted id.
        self.placements.retain(|_, project| *project != id);
        Ok(removed)
    }

    /// Move a session and everything launched under it.
    ///
    /// A subtree shares one project, so moving a child on its own is refused
    /// rather than quietly splitting it away from its parent.
    pub fn assign(
        &mut self,
        session_id: SessionId,
        project_id: Option<ProjectId>,
        live: &[SessionSummary],
    ) -> Result<(), ErrorCode> {
        if let Some(project) = project_id
            && !self.projects.contains_key(&project)
        {
            return Err(ErrorCode::ProjectNotFound);
        }
        self.prune(live);
        let session = live
            .iter()
            .find(|candidate| candidate.id == session_id)
            .ok_or(ErrorCode::SessionNotFound)?;
        // The parent decides for its subtree. If the parent has already been
        // reaped this is a root, and moving it is ordinary.
        if let Some(parent) = session.parent_session_id
            && live.iter().any(|candidate| candidate.id == parent)
            && self.placements.get(&parent).copied() != project_id
        {
            return Err(ErrorCode::ProjectMismatch);
        }
        for member in subtree(session_id, live) {
            match project_id {
                Some(project) => {
                    self.placements.insert(member, project);
                }
                None => {
                    self.placements.remove(&member);
                }
            }
        }
        Ok(())
    }

    /// Where a session about to be created belongs.
    pub fn resolve_placement(
        &mut self,
        requested: Option<ProjectId>,
        parent_session_id: Option<SessionId>,
        live: &dyn Fn() -> Vec<SessionSummary>,
    ) -> Result<Option<ProjectId>, ErrorCode> {
        if let Some(project) = requested
            && !self.projects.contains_key(&project)
        {
            return Err(ErrorCode::ProjectNotFound);
        }
        let Some(parent) = parent_session_id else {
            return Ok(requested);
        };
        self.prune(&live());
        let inherited = self.placements.get(&parent).copied();
        match requested {
            // A child inherits its parent's project. Saying nothing is how
            // every ordinary launch stays in the same folder.
            None => Ok(inherited),
            Some(project) if inherited == Some(project) => Ok(inherited),
            Some(_) => Err(ErrorCode::ProjectMismatch),
        }
    }

    pub fn place(&mut self, session_id: SessionId, project_id: ProjectId) {
        self.placements.insert(session_id, project_id);
    }

    pub fn placement(&self, session_id: SessionId) -> Option<ProjectId> {
        self.placements.get(&session_id).copied()
    }

    /// Every live session one Koru workspace owns.
    ///
    /// **The join nobody had written down.** A session carries a project, a
    /// project carries an optional workspace scope, and nothing carries the
    /// workspace itself — so this is the only place the two ends meet, and it
    /// meets them in one lock rather than a caller taking two.
    ///
    /// **A session in no project, or in a project with no scope, is never
    /// here.** An unscoped project "belongs to no workspace and is visible
    /// everywhere" (`WorkspaceScope`'s own note), and a session nobody placed
    /// is drawn under a virtual bucket that is not a workspace at all.
    /// Removing one workspace must not close a terminal that belongs to none:
    /// there would be no workspace to restore it with, and no confirmation
    /// could have named it.
    ///
    /// Ordered by session id so two calls answer in the same order, and
    /// filtered to sessions that are actually alive — a reaped row would be a
    /// terminal reported as closed by this operation that had already exited
    /// on its own.
    pub fn sessions_in_scope(
        &self,
        scope: WorkspaceScope,
        live: &[SessionSummary],
    ) -> Vec<SessionId> {
        let owned: BTreeSet<_> = self
            .projects
            .values()
            .filter(|project| project.workspace_scope == Some(scope))
            .map(|project| project.id)
            .collect();
        if owned.is_empty() {
            return Vec::new();
        }
        let mut affected: Vec<_> = live
            .iter()
            .filter(|session| {
                matches!(
                    session.process,
                    ProcessState::Starting | ProcessState::Running
                )
            })
            .filter(|session| {
                self.placements
                    .get(&session.id)
                    .is_some_and(|project| owned.contains(project))
            })
            .map(|session| session.id)
            .collect();
        affected.sort_by_key(|id| id.as_uuid());
        affected
    }

    /// **Make every project that names this workspace name this profile.**
    ///
    /// The repair a workspace changing owner needs (SP-15a). A project's scope
    /// is a `{ profile, workspace }` pair; when the workspace moves, the
    /// profile half of every project naming it stops being true, and the
    /// surface's derived visibility puts those projects in the
    /// missing-workspace area rather than pretending. This is what makes them
    /// right, and it is a project mutation of exactly one field — the
    /// workspace half never changes, because the workspace is the same
    /// workspace.
    ///
    /// **It is a statement of the state to be in, not a diff**, which is the
    /// whole of its idempotence. It names the profile the workspace is now
    /// under and says every project scoped to that workspace should say so;
    /// running it twice changes nothing the second time, and running it after
    /// the host was down for the move repairs whatever is left rather than
    /// replaying a move that already happened. A verb naming the *old* profile
    /// would be a diff, and a diff applied to state that has moved on is how a
    /// stale replay puts something back.
    ///
    /// Answers only the projects it actually changed. One already correct is
    /// not news, and a caller reporting "four projects moved" after a repair
    /// that moved none would be reporting the first attempt's truth.
    pub fn rescope_workspace(
        &mut self,
        workspace: CatalogWorkspaceId,
        profile: CatalogProfileId,
    ) -> Vec<ProjectId> {
        let mut rescoped: Vec<_> = self
            .projects
            .values_mut()
            .filter(|project| {
                project
                    .workspace_scope
                    .is_some_and(|scope| scope.workspace == workspace && scope.profile != profile)
            })
            .map(|project| {
                project.workspace_scope = Some(WorkspaceScope { profile, workspace });
                project.id
            })
            .collect();
        rescoped.sort_by_key(|id| id.as_uuid());
        rescoped
    }

    fn edit(
        &mut self,
        id: ProjectId,
        change: impl FnOnce(&mut ProjectSummary),
    ) -> Result<ProjectSummary, ErrorCode> {
        let project = self
            .projects
            .get_mut(&id)
            .ok_or(ErrorCode::ProjectNotFound)?;
        change(project);
        Ok(project.clone())
    }

    /// Forget sessions the host no longer knows about, so a long-lived
    /// registry does not accumulate placements for ids nothing can name.
    fn prune(&mut self, live: &[SessionSummary]) {
        if self.placements.is_empty() {
            return;
        }
        let known: BTreeSet<_> = live.iter().map(|session| session.id).collect();
        self.placements.retain(|session, _| known.contains(session));
    }
}

struct Guarded {
    state: ProjectState,
    subscribers: BTreeMap<SubscriptionId, mpsc::SyncSender<CatalogEvent>>,
}

/// The device-local project authority: [`ProjectState`] plus its file, its
/// invalidation stream, and the two registry events.
pub struct ProjectRegistry {
    guarded: Mutex<Guarded>,
    write: AtomicPath,
    quarantine: PathBuf,
    lifecycle: Arc<dyn SessionLifecycleSink>,
}

/// A resolved placement, held open across session creation.
///
/// The lock is deliberately still held: creation and placement have to be one
/// step, or a window can see a session that briefly belongs to no project and
/// a retry can race the assignment. Dropping this without [`Self::commit`] —
/// which is what a failed spawn does — records nothing.
pub struct Placement<'registry> {
    guarded: MutexGuard<'registry, Guarded>,
    project: Option<ProjectId>,
}

impl Placement<'_> {
    pub fn project(&self) -> Option<ProjectId> {
        self.project
    }

    pub fn commit(mut self, session_id: SessionId) {
        if let Some(project) = self.project {
            self.guarded.state.place(session_id, project);
        }
    }
}

impl ProjectRegistry {
    /// Dispatch fixtures need project reads without opening any real file.
    #[cfg(test)]
    pub(crate) fn in_memory(paths: &SessionPaths) -> Self {
        Self {
            guarded: Mutex::new(Guarded {
                state: ProjectState::restored(Vec::new()),
                subscribers: BTreeMap::new(),
            }),
            write: paths.projects_atomic_path(),
            quarantine: paths.projects_quarantine_path(),
            lifecycle: Arc::new(koru_session_runtime::NoopLifecycleSink),
        }
    }

    /// Restore the registry from its file, or start empty.
    ///
    /// A file that cannot be decoded is *copied* aside before this host ever
    /// writes: recovery must not be able to destroy the thing it was called
    /// to preserve.
    pub fn open(paths: &SessionPaths, lifecycle: Arc<dyn SessionLifecycleSink>) -> Self {
        let file = paths.projects_file();
        let quarantine = paths.projects_quarantine_path();
        let projects = match std::fs::read(&file) {
            Ok(bytes) => match parse(&bytes) {
                Ok(restored) => {
                    if restored.skipped_rows > 0 {
                        tracing::warn!(
                            target: "koru::session_host",
                            skipped_rows = restored.skipped_rows,
                            "project registry rows were unreadable and were skipped"
                        );
                    }
                    restored.projects
                }
                Err(error) => {
                    let preserved = std::fs::copy(&file, &quarantine).is_ok();
                    tracing::warn!(
                        target: "koru::session_host",
                        %error,
                        preserved,
                        "project registry could not be read; starting empty"
                    );
                    Vec::new()
                }
            },
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => Vec::new(),
            Err(error) => {
                tracing::warn!(
                    target: "koru::session_host",
                    %error,
                    "project registry could not be opened; starting empty"
                );
                Vec::new()
            }
        };
        Self {
            guarded: Mutex::new(Guarded {
                state: ProjectState::restored(projects),
                subscribers: BTreeMap::new(),
            }),
            write: paths.projects_atomic_path(),
            quarantine,
            lifecycle,
        }
    }

    pub fn quarantine_path(&self) -> &Path {
        &self.quarantine
    }

    pub fn list(&self) -> Vec<ProjectSummary> {
        self.lock().state.list()
    }

    pub fn create(
        &self,
        request: CreateProject,
        created_at_ms: i64,
    ) -> Result<ProjectSummary, ErrorCode> {
        let mut guarded = self.lock();
        let created = guarded.state.create(request, created_at_ms)?;
        if created.replayed {
            return Ok(created.project);
        }
        self.commit(guarded)?;
        self.lifecycle.publish(
            TerminalProjectCreated {
                scope: scope_token(&created.project),
                root: match created.project.root_directory {
                    Some(_) => TerminalProjectRoot::Set,
                    None => TerminalProjectRoot::Unset,
                },
            }
            .into(),
        );
        Ok(created.project)
    }

    pub fn rename(&self, id: ProjectId, name: String) -> Result<ProjectSummary, ErrorCode> {
        let mut guarded = self.lock();
        let project = guarded.state.rename(id, name)?;
        self.commit(guarded)?;
        Ok(project)
    }

    pub fn set_root(
        &self,
        id: ProjectId,
        root_directory: Option<String>,
    ) -> Result<ProjectSummary, ErrorCode> {
        let mut guarded = self.lock();
        let project = guarded.state.set_root(id, root_directory)?;
        self.commit(guarded)?;
        Ok(project)
    }

    /// **Move one project to a workspace** (protocol v13).
    ///
    /// One typed operation applied to the current state under the one lock,
    /// committed and then invalidated — the module's rule, and the reason a
    /// window never hands the host a registry it read a moment ago. Two windows
    /// moving the same project serialize here, and the last one to commit is
    /// what the file and every subscriber then hold.
    ///
    /// **A move that changes nothing writes nothing and tells nobody.** That is
    /// what makes naming a destination idempotent in practice as well as in
    /// shape: a retry, a replay and a second person choosing the workspace the
    /// project is already in are the same call, and none of them should have
    /// windows re-reading a registry that is the same registry.
    /// [`ProjectState::rescope_workspace`] takes exactly this position.
    ///
    /// The row is answered either way, because "it is already there" and "I put
    /// it there" are the same answer to a caller asking where it is.
    pub fn set_scope(
        &self,
        id: ProjectId,
        workspace_scope: Option<WorkspaceScope>,
    ) -> Result<ProjectSummary, ErrorCode> {
        let mut guarded = self.lock();
        let moved = guarded.state.set_scope(id, workspace_scope)?;
        if !moved.changed {
            return Ok(moved.project);
        }
        self.commit(guarded)?;
        Ok(moved.project)
    }

    pub fn delete(
        &self,
        id: ProjectId,
        sessions: &dyn Fn() -> Vec<SessionSummary>,
    ) -> Result<(), ErrorCode> {
        let mut guarded = self.lock();
        let removed = guarded.state.delete(id, &sessions())?;
        self.commit(guarded)?;
        self.lifecycle.publish(
            TerminalProjectDeleted {
                scope: scope_token(&removed),
            }
            .into(),
        );
        Ok(())
    }

    pub fn assign(
        &self,
        session_id: SessionId,
        project_id: Option<ProjectId>,
        sessions: &dyn Fn() -> Vec<SessionSummary>,
    ) -> Result<(), ErrorCode> {
        let mut guarded = self.lock();
        guarded.state.assign(session_id, project_id, &sessions())?;
        // Placement is memory-only, so there is nothing to write — but the
        // tree that groups sessions by project is now wrong everywhere else.
        self.notify(&guarded);
        Ok(())
    }

    /// Resolve where a session about to be created belongs, holding the
    /// decision open until the session exists.
    pub fn place_at_create(
        &self,
        requested: Option<ProjectId>,
        parent_session_id: Option<SessionId>,
        sessions: &dyn Fn() -> Vec<SessionSummary>,
    ) -> Result<Placement<'_>, ErrorCode> {
        let mut guarded = self.lock();
        let project = guarded
            .state
            .resolve_placement(requested, parent_session_id, sessions)?;
        Ok(Placement { guarded, project })
    }

    pub fn placement(&self, session_id: SessionId) -> Option<ProjectId> {
        self.lock().state.placement(session_id)
    }

    /// Which live sessions a workspace owns. See
    /// [`ProjectState::sessions_in_scope`] for the join and the unscoped rule.
    pub fn sessions_in_scope(
        &self,
        scope: WorkspaceScope,
        live: &[SessionSummary],
    ) -> Vec<SessionId> {
        self.lock().state.sessions_in_scope(scope, live)
    }

    /// **Repair every project that names a workspace whose owner changed.**
    ///
    /// One typed mutation under one lock, committed and then invalidated —
    /// ADR-264's rule, and the reason this is a verb rather than something the
    /// host works out for itself: mutations serialize here as typed
    /// operations, and `ProjectsChanged` is the invalidation *after* an
    /// authoritative mutation, never the thing that causes one.
    ///
    /// A repair that changed nothing writes nothing and tells nobody. There is
    /// no file to rewrite and no window to invalidate — and a notification
    /// after every retry would have windows re-reading a registry that is the
    /// same registry.
    pub fn rescope_workspace(
        &self,
        workspace: CatalogWorkspaceId,
        profile: CatalogProfileId,
    ) -> Result<Vec<ProjectId>, ErrorCode> {
        let mut guarded = self.lock();
        let rescoped = guarded.state.rescope_workspace(workspace, profile);
        if rescoped.is_empty() {
            return Ok(rescoped);
        }
        self.commit(guarded)?;
        Ok(rescoped)
    }

    /// Stamp placement onto summaries as they leave the host. The runtime
    /// builds a summary; the project registry is what knows this part of it.
    pub fn decorate(&self, sessions: Vec<SessionSummary>) -> Vec<SessionSummary> {
        let guarded = self.lock();
        sessions
            .into_iter()
            .map(|session| {
                let project = guarded.state.placement(session.id);
                session.with_project(project)
            })
            .collect()
    }

    /// Install one catalog subscriber. The queue holds a single pending
    /// invalidation; see [`CATALOG_QUEUE`].
    pub fn subscribe(&self, id: SubscriptionId) -> mpsc::Receiver<CatalogEvent> {
        let (send, receive) = mpsc::sync_channel(CATALOG_QUEUE);
        self.lock().subscribers.insert(id, send);
        receive
    }

    /// Drop a subscriber. The sender goes with it, which is what ends the
    /// forwarding thread's blocking receive.
    pub fn unsubscribe(&self, id: SubscriptionId) {
        self.lock().subscribers.remove(&id);
    }

    /// Publish the new state: write the file, then tell everybody. In that
    /// order, so nothing is invalidated toward a registry that did not land.
    fn commit(&self, guarded: MutexGuard<'_, Guarded>) -> Result<(), ErrorCode> {
        write_atomic(&self.write, &encode(&guarded.state.list())).map_err(|error| {
            tracing::warn!(
                target: "koru::session_host",
                %error,
                "the project registry could not be written"
            );
            ErrorCode::Internal
        })?;
        self.notify(&guarded);
        Ok(())
    }

    fn notify(&self, guarded: &Guarded) {
        for sender in guarded.subscribers.values() {
            // A full queue already holds this same notification.
            let _ = sender.try_send(CatalogEvent::ProjectsChanged);
        }
    }

    fn lock(&self) -> MutexGuard<'_, Guarded> {
        self.guarded.lock().unwrap_or_else(|held| held.into_inner())
    }
}

fn scope_token(project: &ProjectSummary) -> TerminalProjectScope {
    match project.workspace_scope {
        Some(_) => TerminalProjectScope::Workspace,
        None => TerminalProjectScope::Unscoped,
    }
}

/// A session and everything launched under it. Bounded by the session list,
/// so a parent chain that somehow cycled cannot spin here.
fn subtree(root: SessionId, sessions: &[SessionSummary]) -> BTreeSet<SessionId> {
    let mut members = BTreeSet::from([root]);
    let mut frontier = VecDeque::from([root]);
    let mut steps = 0;
    while let Some(parent) = frontier.pop_front() {
        steps += 1;
        if steps > sessions.len() + 1 {
            break;
        }
        for child in sessions
            .iter()
            .filter(|session| session.parent_session_id == Some(parent))
        {
            if members.insert(child.id) {
                frontier.push_back(child.id);
            }
        }
    }
    members
}

#[cfg(test)]
mod tests;
