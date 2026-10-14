-- runs.db schema 1 -> 2.
--
-- Embedded verbatim by crates/koru-delegate-sqlite. Production migration code
-- owns the transaction and the PRAGMA user_version compatibility stamp.
--
-- What a person decided when they put a run in the trash, held until the trash
-- is emptied. Trashing a run changes nothing on disk and nothing in the
-- artifact store -- it is reversible, and the run comes back out whole -- so
-- the only thing there is to remember in between is the answer to the one
-- question a trash asks: the run left results behind, do you want them kept or
-- deleted with it?
--
-- Keyed by run id and by nothing else, with no foreign key to run_intents. A
-- run that reached this device by sync has no creation record here and nothing
-- device-local to delete; recording its decision anyway is harmless, and a
-- constraint that refused it would turn "there is nothing to do" into an error
-- a person would see. Emptying the trash removes the row along with every other
-- row of the run.
CREATE TABLE run_deletions (
  run_id         TEXT PRIMARY KEY,
  artifacts      TEXT NOT NULL CHECK (artifacts IN ('keep', 'delete')),
  decided_at_ms  INTEGER NOT NULL
);
