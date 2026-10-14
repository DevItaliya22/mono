-- artifacts.db schema 1 -> 2.
--
-- Embedded verbatim by crates/koru-artifact-sqlite. Production migration code
-- owns the transaction and the PRAGMA user_version compatibility stamp.
--
-- The run authority record (ROADMAP §7.3, D28): what a delegated run may do,
-- resolved server-side from the token it presents. One row per run; the
-- grant set is a child table rather than an encoded string (ADR-308); every
-- token ever issued is kept, retired by a mark when a later attempt replaces
-- it, so a stale token is refused as stale and not as unknown.

CREATE TABLE run_authorities (
  run_id             TEXT PRIMARY KEY,         -- RunId
  workspace_id       TEXT NOT NULL,            -- the run's own workspace: where a created result lands
  attempt_generation INTEGER NOT NULL CHECK (attempt_generation >= 1),
  run_workspace_path TEXT NOT NULL,            -- content: read by the register path rules, in no log or event
  state              TEXT NOT NULL CHECK (state IN ('active', 'revoked')),
  created_at_ms      INTEGER NOT NULL,
  revoked_at_ms      INTEGER                   -- revoke, never erase
);
-- D28: the workspaces the run may reach, replaced as a complete set and
-- frozen for an attempt. Empty means no workspace access.
CREATE TABLE run_authority_grants (
  run_id             TEXT NOT NULL REFERENCES run_authorities(run_id),
  workspace_id       TEXT NOT NULL,
  PRIMARY KEY (run_id, workspace_id)
);
-- Every endpoint token issued, by the hash of the bearer. The current one
-- is the row with retired_at_ms NULL; advancing the generation retires it
-- and inserts the next. The bearer itself is never stored.
CREATE TABLE run_authority_tokens (
  token_hash         TEXT PRIMARY KEY,
  run_id             TEXT NOT NULL REFERENCES run_authorities(run_id),
  attempt_generation INTEGER NOT NULL CHECK (attempt_generation >= 1),
  issued_at_ms       INTEGER NOT NULL,
  retired_at_ms      INTEGER
);
CREATE INDEX run_authority_tokens_by_run
  ON run_authority_tokens(run_id) WHERE retired_at_ms IS NULL;
