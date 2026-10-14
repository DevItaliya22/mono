-- runs.db schema 0 -> 1.
--
-- Embedded verbatim by crates/koru-delegate-sqlite. Production migration code
-- owns the transaction and the PRAGMA user_version compatibility stamp.
--
-- The device-local execution ledger of delegated runs: how far *this machine*
-- got (see docs/architecture/database/README.md). The Run document is the run and
-- holds its one canonical status; nothing here is a second one, and nothing
-- here syncs. Closed domains are CHECKed the way 016_agent_profile_cleanup.sql
-- does it, because SQLite has no enum. Every time is epoch milliseconds UTC
-- with the unit in the column name.
--
-- payload_json and body_json are the two columns that carry content: the
-- delegation payload a restart completes a run from, and the envelope a
-- retried publication re-sends. They are read by the worker and by nothing
-- else -- never a log, an event attribute, a Debug rendering or an error.

-- 7.1 step 1: the creation intent, with the whole delegation payload a restart
-- needs to finish the job. Pending until the Run document is committed; listed
-- as a request and never as a run.
CREATE TABLE run_intents (
  run_id             TEXT PRIMARY KEY,
  workspace_id       TEXT NOT NULL,
  payload_json       TEXT NOT NULL,
  state              TEXT NOT NULL CHECK (state IN ('pending', 'satisfied', 'abandoned')),
  run_document_id    TEXT,
  created_at_ms      INTEGER NOT NULL,
  updated_at_ms      INTEGER NOT NULL,
  CHECK (state <> 'satisfied' OR run_document_id IS NOT NULL)
);

-- Execution-attempt state, one row per generation. `retired` is the fence of
-- 7.6: a fresh attempt opens only once the one before it is retired, and a
-- delayed create carrying a retired key is refused. `attempt_id` is the
-- RunAttemptId every turn and timeline entry names, so a reattached attempt
-- is the same attempt after a restart. There is deliberately no 'failed'
-- state (D10): a restart interrupts, it never fails.
CREATE TABLE run_attempts (
  run_id             TEXT NOT NULL REFERENCES run_intents(run_id),
  generation         INTEGER NOT NULL CHECK (generation >= 1),
  attempt_id         TEXT NOT NULL UNIQUE,
  state              TEXT NOT NULL CHECK (state IN (
    'created', 'starting', 'running', 'interrupted', 'ended', 'unknown'
  )),
  session_key        TEXT UNIQUE,
  session_id         TEXT,
  host_instance_id   TEXT,
  retired            INTEGER NOT NULL DEFAULT 0 CHECK (retired IN (0, 1)),
  started_at_ms      INTEGER,
  updated_at_ms      INTEGER NOT NULL,
  PRIMARY KEY (run_id, generation)
);

-- The retry queue (7.1): a lost lifecycle publication, a lost artifact
-- registration reply, an economics fact not yet on the bus. The same EventId
-- is reused on retry; the journal dedupes it. Acknowledged rows stay -- a row
-- is a fact -- and the partial index keeps the open set cheap to scan.
CREATE TABLE pending_publications (
  event_id           TEXT PRIMARY KEY,
  run_id             TEXT NOT NULL,
  kind               TEXT NOT NULL CHECK (kind IN (
    'lifecycle', 'artifact_registration', 'usage'
  )),
  body_json          TEXT NOT NULL,
  attempts           INTEGER NOT NULL DEFAULT 0 CHECK (attempts >= 0),
  queued_at_ms       INTEGER NOT NULL,
  acknowledged_at_ms INTEGER
);
CREATE INDEX pending_publications_open
  ON pending_publications(queued_at_ms) WHERE acknowledged_at_ms IS NULL;

-- Where each projection of a run's retained facts has got to (17.1): the
-- timeline index last folded into the header summary and last published.
-- Device-local; the facts themselves live in the Run document and sync.
CREATE TABLE projection_cursors (
  run_id             TEXT NOT NULL,
  projection         TEXT NOT NULL CHECK (projection IN ('header_summary', 'bus')),
  timeline_index     INTEGER NOT NULL CHECK (timeline_index >= 0),
  updated_at_ms      INTEGER NOT NULL,
  PRIMARY KEY (run_id, projection)
);

-- One slot, one run (7.7): the claim and its planned RunId are one row, keyed
-- by the slot and never by the revision, so editing a template can never turn
-- one slot into two. The revision in force when the slot was claimed is
-- recorded on it.
CREATE TABLE schedule_claims (
  schedule_id        TEXT NOT NULL,
  occurrence_ms      INTEGER NOT NULL,
  revision           INTEGER NOT NULL CHECK (revision >= 1),
  run_id             TEXT NOT NULL UNIQUE,
  outcome            TEXT NOT NULL CHECK (outcome IN (
    'claimed', 'fired', 'skipped_missed', 'skipped_busy', 'skipped_retired'
  )),
  claimed_at_ms      INTEGER NOT NULL,
  PRIMARY KEY (schedule_id, occurrence_ms)
);
