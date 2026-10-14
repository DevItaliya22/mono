-- Catalog 10 -> 11. Independent Profile documents; legacy bytes stay intact.
CREATE TABLE profile_identity_crdts (
  profile_id TEXT PRIMARY KEY REFERENCES profiles(id) ON DELETE RESTRICT,
  snapshot BLOB NOT NULL CHECK (length(snapshot) > 0)
);

CREATE TABLE profile_private_crdts (
  profile_id TEXT PRIMARY KEY REFERENCES profiles(id) ON DELETE RESTRICT,
  snapshot BLOB NOT NULL CHECK (length(snapshot) > 0)
);

CREATE TABLE profile_personal_crdts (
  owner_profile_id TEXT NOT NULL REFERENCES profiles(id) ON DELETE RESTRICT,
  workspace_id TEXT NOT NULL REFERENCES workspaces(id) ON DELETE RESTRICT,
  snapshot BLOB NOT NULL CHECK (length(snapshot) > 0),
  indexed_version BLOB,
  PRIMARY KEY (owner_profile_id, workspace_id)
);

CREATE TABLE profile_reminder_due (
  owner_profile_id TEXT NOT NULL,
  workspace_id TEXT NOT NULL,
  reminder_id TEXT NOT NULL,
  due_at_ms INTEGER,
  PRIMARY KEY (owner_profile_id, workspace_id, reminder_id),
  FOREIGN KEY (owner_profile_id, workspace_id)
    REFERENCES profile_personal_crdts(owner_profile_id, workspace_id)
    ON DELETE CASCADE
);
CREATE INDEX profile_reminder_due_by_time
  ON profile_reminder_due(owner_profile_id, due_at_ms, workspace_id, reminder_id);
