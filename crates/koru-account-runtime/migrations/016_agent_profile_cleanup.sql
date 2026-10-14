BEGIN IMMEDIATE;
CREATE TABLE IF NOT EXISTS profile_cleanup (
  profile_id             TEXT PRIMARY KEY,
  expected_revision      INTEGER NOT NULL CHECK(expected_revision >= 0),
  lifecycle_state        TEXT NOT NULL CHECK(lifecycle_state IN ('active', 'deleted')),
  lifecycle_revision     INTEGER NOT NULL CHECK(lifecycle_revision >= 0),
  operation_id           TEXT NOT NULL UNIQUE,
  stage                  TEXT NOT NULL CHECK(stage IN (
    'hosted_pending', 'hosted_committed', 'workspaces_removed',
    'profile_removed', 'settings_cleared', 'navigation_settled'
  )),
  peer_targets_json      TEXT NOT NULL,
  failure_code           TEXT,
  updated_at_millis      INTEGER NOT NULL
);
CREATE TABLE IF NOT EXISTS profile_restore (
  profile_id             TEXT PRIMARY KEY,
  expected_revision      INTEGER NOT NULL CHECK(expected_revision >= 1),
  operation_id           TEXT NOT NULL UNIQUE,
  lifecycle_revision     INTEGER CHECK(lifecycle_revision >= 2),
  failure_code           TEXT,
  updated_at_millis      INTEGER NOT NULL
);
PRAGMA user_version = 2;
COMMIT;
