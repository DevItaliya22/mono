-- Projection schema 5 -> 6: a real per-document modified-at, and an index that
-- can group tags by value.
--
-- `documents` gains `content_digest` and `modified_at`. The sort formerly
-- called "modified" ordered by `projected_at` — *when the projector last ran* —
-- so a rebuild restamped every document and collapsed the order. The new pair
-- moves only when the projected content moves, the same fingerprint rule the
-- search index applies to `observed_modified_at_ms`.
--
-- `attrs` gains `(role, value)`. The existing `attrs_role` is `(role, kind)`
-- and cannot serve a GROUP BY on the value, which is what counting a tag over a
-- whole corpus is.
--
-- Projection is a derived cache of the authoritative workspace documents, so
-- its migration is a rebuild. User content is not stored in this database.
--
-- **Version-agnostic on purpose.** This is the only migration the projection
-- runs, from any stored version to the current one, because a rebuild does not
-- care where it started. A future bump replaces the file this points at rather
-- than adding a step to a chain.
DROP TABLE IF EXISTS attrs;
DROP TABLE IF EXISTS runs;
DROP TABLE IF EXISTS nodes;
DROP TABLE IF EXISTS documents;
DROP TABLE IF EXISTS source_status;
DROP TABLE IF EXISTS listing;
DROP TABLE IF EXISTS workspaces;
DROP TABLE IF EXISTS state;
