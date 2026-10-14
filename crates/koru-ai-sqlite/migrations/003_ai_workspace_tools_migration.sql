-- AI schema 2 -> 3: per-chat workspace grants and content-free tool audit.

ALTER TABLE chats ADD COLUMN tool_mode TEXT NOT NULL DEFAULT 'manual'
    CHECK (tool_mode IN ('manual', 'auto'));

CREATE TABLE chat_workspace_grants (
    chat_id          TEXT NOT NULL REFERENCES chats(id) ON DELETE CASCADE,
    workspace_id     TEXT NOT NULL,
    granted_at_ms    INTEGER NOT NULL,
    PRIMARY KEY (chat_id, workspace_id),
    CHECK (length(workspace_id) > 0)
);

-- Arguments, results, queries, document content, titles, and paths are
-- intentionally absent. This row is authorization/audit state, not a replay
-- log for provider or Store payloads.
CREATE TABLE tool_invocations (
    id                TEXT PRIMARY KEY,
    attempt_id        TEXT NOT NULL REFERENCES inference_attempts(id) ON DELETE CASCADE,
    chat_id           TEXT NOT NULL REFERENCES chats(id) ON DELETE CASCADE,
    tool_name         TEXT NOT NULL,
    read_only         INTEGER NOT NULL CHECK (read_only IN (0, 1)),
    status            TEXT NOT NULL CHECK (status IN (
        'proposed', 'approval_required', 'running', 'complete',
        'denied', 'failed', 'cancelled'
    )),
    approval          TEXT NOT NULL CHECK (approval IN (
        'not_required', 'pending', 'approved', 'denied'
    )),
    proposed_at_ms    INTEGER NOT NULL,
    started_at_ms     INTEGER,
    completed_at_ms   INTEGER,
    result_bytes      INTEGER CHECK (result_bytes IS NULL OR result_bytes >= 0),
    error_code        TEXT CHECK (
        error_code IS NULL OR error_code IN (
            'permission_denied', 'execution_failed', 'cancelled'
        )
    ),
    CHECK (length(id) > 0),
    CHECK (length(tool_name) > 0),
    CHECK (started_at_ms IS NULL OR started_at_ms >= proposed_at_ms),
    CHECK (completed_at_ms IS NULL OR completed_at_ms >= proposed_at_ms),
    CHECK (
        status NOT IN ('complete', 'denied', 'failed', 'cancelled') OR
        completed_at_ms IS NOT NULL
    )
);

CREATE TRIGGER tool_invocation_terminal_state_is_absorbing
BEFORE UPDATE OF status ON tool_invocations
WHEN OLD.status IN ('complete', 'denied', 'failed', 'cancelled')
 AND NEW.status <> OLD.status
BEGIN
    SELECT RAISE(ABORT, 'terminal tool invocation state is absorbing');
END;

CREATE INDEX chat_workspace_grants_by_workspace
    ON chat_workspace_grants(workspace_id, chat_id);
CREATE INDEX tool_invocations_by_chat
    ON tool_invocations(chat_id, proposed_at_ms, id);
CREATE INDEX tool_invocations_by_attempt
    ON tool_invocations(attempt_id, proposed_at_ms, id);
