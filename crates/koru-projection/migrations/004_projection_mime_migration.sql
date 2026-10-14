-- Projection schema 3 -> 4.
--
-- `listing.mime` was added while the version remained 3. A database already
-- stamped 3 therefore kept its old table because the current schema uses
-- `CREATE TABLE IF NOT EXISTS`, and every listing update failed with "table
-- listing has no column named mime".
--
-- Projection is a derived cache of the authoritative workspace documents, so
-- its migration is a rebuild. User content is not stored in this database.
DROP TABLE IF EXISTS attrs;
DROP TABLE IF EXISTS runs;
DROP TABLE IF EXISTS nodes;
DROP TABLE IF EXISTS documents;
DROP TABLE IF EXISTS source_status;
DROP TABLE IF EXISTS listing;
DROP TABLE IF EXISTS workspaces;
DROP TABLE IF EXISTS state;
