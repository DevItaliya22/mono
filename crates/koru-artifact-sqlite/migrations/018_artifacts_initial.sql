-- artifacts.db schema 0 -> 1.
--
-- Embedded verbatim by crates/koru-artifact-sqlite. Production migration code
-- owns the transaction and the PRAGMA user_version compatibility stamp.
--
-- Identity separate from content, versions immutable: `version_id` is an
-- ArtifactVersionId, `blob_hash` is the address of the bytes in the
-- workspace's content-addressed blob store, and the trigger below is what
-- refuses a rewrite (docs/architecture/database/README.md, Artifact store SQLite).

CREATE TABLE artifact_families (
  family_id          TEXT PRIMARY KEY,
  workspace_id       TEXT NOT NULL,
  title              TEXT NOT NULL,
  created_by_run_id  TEXT,
  created_at_ms      INTEGER NOT NULL,
  retired_at_ms      INTEGER                   -- retire, never erase
);
CREATE INDEX artifact_families_by_workspace
  ON artifact_families(workspace_id) WHERE retired_at_ms IS NULL;
CREATE TABLE artifact_versions (
  version_id         TEXT PRIMARY KEY,         -- ArtifactVersionId
  family_id          TEXT NOT NULL REFERENCES artifact_families(family_id),
  ordinal            INTEGER NOT NULL CHECK (ordinal >= 1),
  blob_hash          TEXT NOT NULL,            -- BlobId, in the workspace's content-addressed store
  size_bytes         INTEGER NOT NULL CHECK (size_bytes >= 0),
  mime               TEXT NOT NULL,
  filename           TEXT,
  produced_by_run_id TEXT,
  produced_by_generation INTEGER CHECK (produced_by_generation IS NULL OR produced_by_generation >= 1),
  registration_id    TEXT NOT NULL UNIQUE,     -- the registration operation id: a retry is the same version
  derived_from       TEXT REFERENCES artifact_versions(version_id),
  created_at_ms      INTEGER NOT NULL,
  UNIQUE (family_id, ordinal)
);
CREATE TRIGGER artifact_versions_are_immutable
  BEFORE UPDATE ON artifact_versions
  BEGIN SELECT RAISE(ABORT, 'artifact_version_immutable'); END;
-- The permission vocabulary, modeled now and owner-only reachable (D11).
CREATE TABLE artifact_permissions (
  family_id          TEXT NOT NULL REFERENCES artifact_families(family_id),
  principal          TEXT NOT NULL,
  access             TEXT NOT NULL CHECK (access IN ('owner', 'read')),
  granted_at_ms      INTEGER NOT NULL,
  PRIMARY KEY (family_id, principal)
);
-- Which run consumed or produced which version (provenance; D6, M3).
CREATE TABLE artifact_run_links (
  run_id             TEXT NOT NULL,
  version_id         TEXT NOT NULL REFERENCES artifact_versions(version_id),
  role               TEXT NOT NULL CHECK (role IN ('input', 'output')),
  linked_at_ms       INTEGER NOT NULL,
  PRIMARY KEY (run_id, version_id, role)
);
