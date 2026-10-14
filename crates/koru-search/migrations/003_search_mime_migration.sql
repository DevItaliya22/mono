-- Search schema 2 -> 3.
--
-- `listing.mime` was added while the version remained 2. A database already
-- stamped 2 therefore kept its old table because the current schema uses
-- `CREATE TABLE IF NOT EXISTS`, and every listing read failed with "no such
-- column: mime".
--
-- Search is a derived cache of the projection, so its migration is a rebuild:
-- discard every search-owned table and let the current schema plus reconcile
-- repopulate it. User content is not stored in this database.
DROP TRIGGER IF EXISTS fragments_ai;
DROP TRIGGER IF EXISTS fragments_ad;
DROP TRIGGER IF EXISTS fragments_au;
DROP TABLE IF EXISTS fragment_fts;
DROP TABLE IF EXISTS fragments;
DROP TABLE IF EXISTS documents;
DROP TABLE IF EXISTS listing;
DROP TABLE IF EXISTS workspaces;
DROP TABLE IF EXISTS recent_targets;
DROP TABLE IF EXISTS recent_queries;
DROP TABLE IF EXISTS state;
