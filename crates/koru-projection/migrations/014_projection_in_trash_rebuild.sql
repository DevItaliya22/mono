-- Projection schema 6 -> 7: a binned node is projected, and says so.
--
-- `nodes` gains `in_trash`. The projector used to walk `WorkspaceDoc::walk`,
-- which omits the document's hidden trash root, so a binned node simply left
-- the read model — and a surface that has to offer *restore* cannot build a row
-- for something it cannot see. The walk is now `walk_with_trash` and every node
-- carries the flag, so a reader chooses: live content by default, everything
-- when it is the bin it is drawing.
--
-- Projection is a derived cache of the authoritative workspace documents, so
-- its migration is a rebuild. User content is not stored in this database.
--
-- **Version-agnostic on purpose.** This is the only migration the projection
-- runs, from any stored version to the current one, because a rebuild does not
-- care where it started. Its predecessor (`012_projection_modified_at_migration`)
-- carried a `5 -> 6` title and went on serving `6 -> 7` for exactly that
-- reason; this file replaces it rather than joining a chain, and the next bump
-- replaces this one.
DROP TABLE IF EXISTS attrs;
DROP TABLE IF EXISTS runs;
DROP TABLE IF EXISTS nodes;
DROP TABLE IF EXISTS documents;
DROP TABLE IF EXISTS source_status;
DROP TABLE IF EXISTS listing;
DROP TABLE IF EXISTS workspaces;
DROP TABLE IF EXISTS state;
