-- runs.db schema 3: make a skipped-busy schedule fact replayable after a
-- crash. Existing rows migrate as-is; a legacy skipped_busy row without its
-- holder is reported corrupt rather than assigned invented history.

ALTER TABLE schedule_claims
ADD COLUMN busy_holder_run_id TEXT
CHECK (busy_holder_run_id IS NULL OR outcome = 'skipped_busy');
