-- Projection schema 7 -> 8: the tasks query gets its covering index.
--
-- `attrs` gains `attrs_probe (document_id, node_id, role, value, kind)`. Every
-- predicate the tasks query is made of is a correlated `EXISTS` over `attrs`,
-- correlated on `(document_id, node_id)` and then testing `role`, usually
-- `value` and sometimes `kind` (`attr_exists` in `store.rs` is the one builder,
-- so there is one shape to serve). `attrs_node` could seek the node's attribute
-- rows but not test the rest, so each probe finished with a random **table row**
-- read, once per attribute per candidate node, and the candidate set is the
-- workspace. With the three columns carried along the same probes are answered
-- from the index and touch no table row at all: on the roadmap corpus a scoped
-- page falls from 220 ms to 48 ms and a Tasks tab's three statements from 830 ms
-- to 114 ms (ADR-250; `docs/features/tasks/PERF_BASELINE.md` §5, "After the index").
--
-- Projection is a derived cache of the authoritative workspace documents, so
-- its migration is a rebuild. User content is not stored in this database.
--
-- **Version-agnostic on purpose.** This is the only migration the projection
-- runs, from any stored version to the current one, because a rebuild does not
-- care where it started. Its predecessor (`014_projection_in_trash_rebuild`)
-- said the same of *its* predecessor; this file replaces it rather than joining
-- a chain, and the next bump replaces this one.
DROP TABLE IF EXISTS attrs;
DROP TABLE IF EXISTS runs;
DROP TABLE IF EXISTS nodes;
DROP TABLE IF EXISTS documents;
DROP TABLE IF EXISTS source_status;
DROP TABLE IF EXISTS listing;
DROP TABLE IF EXISTS workspaces;
DROP TABLE IF EXISTS state;
