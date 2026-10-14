-- Search schema 3 -> 4: a tag is stored by name, not by id.
--
-- `attr:koru.tag` fragments used to carry the `TagId` the node stores. They now
-- carry the tag's *name*, resolved from the workspace's system document at
-- index time (ADR-176). The stored text changed meaning, so every fragment
-- written by an older build is wrong text under a right key — and the fuzzy
-- vocabulary counted its terms.
--
-- Search is a derived cache of the projection, so its migration is a rebuild:
-- discard every search-owned table and let the current schema plus reconcile
-- repopulate it. User content is not stored in this database.
--
-- **Version-agnostic on purpose.** This is the only migration the search index
-- runs, from any stored version to the current one, because a rebuild does not
-- care where it started. A future bump replaces the file it points at rather
-- than adding a step to a chain.
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
