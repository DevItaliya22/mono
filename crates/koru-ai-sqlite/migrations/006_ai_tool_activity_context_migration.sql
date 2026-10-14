-- AI schema 5 -> 6: stable tool targets and bounded public failure context.

-- Raw arguments, results, queries, document titles, paths, and workspace
-- content remain absent. Stable ids let the trusted desktop resolve the
-- current document title through Store, while the bounded error summary is
-- already the public-safe message returned by the tool boundary.
ALTER TABLE tool_invocations ADD COLUMN target_workspace_id TEXT
    CHECK (target_workspace_id IS NULL OR length(target_workspace_id) > 0);
ALTER TABLE tool_invocations ADD COLUMN target_document_id TEXT
    CHECK (target_document_id IS NULL OR length(target_document_id) > 0);
ALTER TABLE tool_invocations ADD COLUMN error_message TEXT
    CHECK (error_message IS NULL OR length(error_message) <= 513);
