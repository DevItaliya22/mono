-- runs.db schema 3 -> 4.
--
-- Embedded verbatim by crates/koru-delegate-sqlite. Production migration code
-- owns the transaction and the PRAGMA user_version compatibility stamp.
--
-- Schedule events cannot use pending_publications: that table requires a
-- RunId, while a skipped occurrence deliberately creates no Run. This is the
-- schedule-shaped half of the same durable outbox mechanism. A row is prepared
-- before the authoritative Schedule-document or claim mutation and becomes
-- publishable only after that mutation is known durable. The envelope in
-- body_json is content and is read only by the publisher.

CREATE TABLE schedule_publications (
  event_id             TEXT PRIMARY KEY,
  schedule_id          TEXT NOT NULL,
  workspace_id         TEXT NOT NULL,
  schedule_document_id TEXT NOT NULL,
  kind                 TEXT NOT NULL CHECK (kind IN (
    'created', 'updated', 'paused', 'fired', 'skipped'
  )),
  revision             INTEGER NOT NULL CHECK (revision >= 1),
  occurrence_ms        INTEGER,
  body_json            TEXT NOT NULL,
  attempts             INTEGER NOT NULL DEFAULT 0 CHECK (attempts >= 0),
  queued_at_ms         INTEGER NOT NULL,
  ready_at_ms          INTEGER,
  acknowledged_at_ms   INTEGER,
  CHECK (
    (kind IN ('created', 'updated', 'paused') AND occurrence_ms IS NULL)
    OR
    (kind IN ('fired', 'skipped') AND occurrence_ms IS NOT NULL)
  ),
  CHECK (kind <> 'created' OR revision = 1),
  CHECK (ready_at_ms IS NOT NULL OR acknowledged_at_ms IS NULL)
);

-- An updated or paused definition is one committed fact per revision. The two
-- kinds are mutually exclusive descriptions of that revision.
CREATE UNIQUE INDEX schedule_publications_definition_fact
  ON schedule_publications(schedule_id, revision)
  WHERE kind IN ('updated', 'paused');

-- One chain has one root even if corrupt input offers a second Created under a
-- different revision.
CREATE UNIQUE INDEX schedule_publications_created_root
  ON schedule_publications(schedule_id)
  WHERE kind = 'created';

-- A slot has one outcome. Revision is evidence recorded on that outcome, never
-- part of occurrence identity (ROADMAP 7.7).
CREATE UNIQUE INDEX schedule_publications_occurrence_outcome
  ON schedule_publications(schedule_id, occurrence_ms)
  WHERE kind IN ('fired', 'skipped');

-- Subject validation and per-schedule retained-history reads must not scan the
-- global outbox as it grows.
CREATE INDEX schedule_publications_schedule
  ON schedule_publications(schedule_id, queued_at_ms, event_id);

-- The publisher reads only committed facts. Prepared-but-not-ready rows have a
-- separate bounded restart scan so reconciliation never scans acknowledged
-- history and never mistakes intent for a fact.
CREATE INDEX schedule_publications_ready
  ON schedule_publications(ready_at_ms, queued_at_ms, event_id)
  WHERE ready_at_ms IS NOT NULL AND acknowledged_at_ms IS NULL;

CREATE INDEX schedule_publications_prepared
  ON schedule_publications(queued_at_ms, event_id)
  WHERE ready_at_ms IS NULL;
