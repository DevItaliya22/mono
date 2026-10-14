-- artifacts.db schema 2 -> 3.
--
-- Embedded verbatim by crates/koru-artifact-sqlite. Production migration code
-- owns the transaction and the PRAGMA user_version compatibility stamp.
--
-- Runs this device has deleted, so a result that outlived one can say so.
--
-- A person may delete a run and keep what it produced. The result stays -- it
-- is a thing in its own right, with its own name -- but its provenance still
-- names the run that made it, and an id that resolves to nothing teaches
-- nothing. The provenance walk reads this table and the sentence says "a run
-- that was deleted" instead.
--
-- Deliberately not inferred from the absence of a run authority: a run that
-- reached this device by sync never had one here either, and "we have no
-- record of it" and "it was deleted" are different facts about somebody's
-- work. The authority row itself goes when the run does -- there is nothing
-- left for it to authorize -- and this row is what stays in its place.
--
-- No path, no title, no objective: an id and a moment.
CREATE TABLE deleted_runs (
  run_id        TEXT PRIMARY KEY,
  deleted_at_ms INTEGER NOT NULL
);
