-- Search schema 4 -> 5: attributes are indexed in the type they were declared
-- with, not only as text.
--
-- Every attribute was already indexed, as a `attr:<role>` fragment carrying its
-- label or its value. A fragment is text, and text answers *contains this
-- word*; it cannot answer *due before this date* or *above this number*. The
-- new `attr_values` table keeps `ProjectedAttr.kind` beside the value, with a
-- numeric column for the two kinds that have one, so a filter can compare
-- within a kind and never across kinds (ADR-189, the three temporal types).
--
-- The fragments are unchanged and still written: `high` typed into the box
-- still finds the block by text. The typed row is what a `koru.priority is
-- high` filter uses. Two writers, two questions.
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
DROP TABLE IF EXISTS attr_values;
DROP TABLE IF EXISTS documents;
DROP TABLE IF EXISTS listing;
DROP TABLE IF EXISTS workspaces;
DROP TABLE IF EXISTS recent_targets;
DROP TABLE IF EXISTS recent_queries;
DROP TABLE IF EXISTS state;
