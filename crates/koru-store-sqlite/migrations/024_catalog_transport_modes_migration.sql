-- Catalog schema 11 -> 12. Device-local transport modes (D230, D249).
--
-- Exactly the DDL approved in the transport-mode catalog amendment (an
-- internal planning record, not published). The row is the
-- authority for "may this Profile or Workspace move data at all, and by which
-- transport" on *this device*: it is never replicated, never exchanged with a
-- peer or the Cloud, and never derived from another mode document.
--
-- The catalog/root supplies the device scope, so no account identity, endpoint
-- key or new persistent device id appears here. The Store's own transaction
-- owns the commit and the schema stamp.
CREATE TABLE local_transport_modes (
    subject_kind TEXT NOT NULL CHECK (subject_kind IN ('profile', 'workspace')),
    subject_id TEXT NOT NULL,
    owner_profile_id TEXT NOT NULL REFERENCES profiles(id) ON DELETE RESTRICT,
    workspace_id TEXT REFERENCES workspaces(id) ON DELETE RESTRICT,
    mode TEXT NOT NULL CHECK (mode IN ('local_only', 'p2p', 'cloud')),
    explicit_choice INTEGER NOT NULL CHECK (explicit_choice IN (0, 1)),
    revision INTEGER NOT NULL CHECK (typeof(revision) = 'integer' AND revision > 0),
    PRIMARY KEY (subject_kind, subject_id),
    CHECK (
        (subject_kind = 'profile' AND subject_id = owner_profile_id AND workspace_id IS NULL)
        OR
        (subject_kind = 'workspace' AND workspace_id IS NOT NULL AND subject_id = workspace_id)
    )
);
CREATE INDEX local_transport_modes_owner_idx
    ON local_transport_modes(owner_profile_id, subject_kind, subject_id);

-- Seed Local-only so an existing installation opens safely before any
-- asynchronous sharing evidence arrives. Every retained row is seeded,
-- including a locally removed Profile and a Workspace tombstone, because a
-- missing row is a storage error rather than a default.
INSERT INTO local_transport_modes
    (subject_kind, subject_id, owner_profile_id, workspace_id, mode, explicit_choice, revision)
SELECT 'profile', id, id, NULL, 'local_only', 0, 1 FROM profiles;
INSERT INTO local_transport_modes
    (subject_kind, subject_id, owner_profile_id, workspace_id, mode, explicit_choice, revision)
SELECT 'workspace', id, owner, id, 'local_only', 0, 1 FROM workspaces;
-- Subsequent verified defaults use the Store's compare-and-set operation.
-- The Store validates owner_profile_id against workspaces.owner on every
-- mutation and updates it atomically with any supported ownership change.
