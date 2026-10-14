-- ai.db schema 0 -> 1.
--
-- Embedded verbatim by crates/koru-ai-sqlite. Production migration code owns
-- the transaction and PRAGMA user_version compatibility stamp.

CREATE TABLE ai_settings (
    singleton              INTEGER PRIMARY KEY CHECK (singleton = 1),
    selected_provider      TEXT CHECK (
        selected_provider IS NULL OR length(selected_provider) > 0
    ),
    openai_model           TEXT,
    openai_key_account     TEXT,
    openai_reasoning       TEXT NOT NULL DEFAULT 'default' CHECK (
        openai_reasoning IN (
            'default', 'off', 'on', 'minimal', 'low', 'medium', 'high', 'xhigh', 'max'
        )
    ),
    ollama_endpoint        TEXT NOT NULL DEFAULT 'http://localhost:11434',
    ollama_model           TEXT,
    ollama_reasoning       TEXT NOT NULL DEFAULT 'default' CHECK (
        ollama_reasoning IN (
            'default', 'off', 'on', 'minimal', 'low', 'medium', 'high', 'xhigh', 'max'
        )
    ),
    created_at_ms          INTEGER NOT NULL,
    updated_at_ms          INTEGER NOT NULL,
    CHECK (updated_at_ms >= created_at_ms)
);

-- There is intentionally no default row. Setup inserts the singleton only
-- after the user chooses OpenAI or Ollama. Provider labels, default endpoints,
-- key requirements and adapters are code-owned product metadata, not rows.

CREATE TABLE chats (
    id                       TEXT PRIMARY KEY,
    title                    TEXT NOT NULL,
    current_provider         TEXT NOT NULL CHECK (length(current_provider) > 0),
    current_model            TEXT NOT NULL CHECK (length(current_model) > 0),
    current_reasoning        TEXT NOT NULL DEFAULT 'default' CHECK (
        current_reasoning IN (
            'default', 'off', 'on', 'minimal', 'low', 'medium', 'high', 'xhigh', 'max'
        )
    ),
    branched_from_chat_id    TEXT REFERENCES chats(id) ON DELETE SET NULL,
    branched_from_message_id TEXT,
    created_at_ms            INTEGER NOT NULL,
    updated_at_ms            INTEGER NOT NULL,
    CHECK (length(id) > 0),
    CHECK (updated_at_ms >= created_at_ms)
);

CREATE TABLE messages (
    id                    TEXT PRIMARY KEY,
    chat_id               TEXT NOT NULL REFERENCES chats(id) ON DELETE CASCADE,
    ordinal               INTEGER NOT NULL CHECK (ordinal >= 0),
    role                  TEXT NOT NULL CHECK (role IN ('user', 'assistant')),
    state                 TEXT NOT NULL CHECK (
        state IN ('pending', 'complete', 'interrupted', 'failed')
    ),
    markdown              TEXT NOT NULL DEFAULT '',
    origin_message_id     TEXT REFERENCES messages(id) ON DELETE SET NULL,
    provider              TEXT CHECK (provider IS NULL OR length(provider) > 0),
    requested_model       TEXT,
    resolved_model        TEXT,
    error_class           TEXT CHECK (
        error_class IS NULL OR error_class IN (
            'configuration', 'authentication', 'rate_limit', 'context_too_large',
            'model_unavailable', 'network', 'provider', 'cancelled', 'storage'
        )
    ),
    created_at_ms         INTEGER NOT NULL,
    completed_at_ms       INTEGER,
    UNIQUE (chat_id, ordinal),
    CHECK (length(id) > 0),
    CHECK (completed_at_ms IS NULL OR completed_at_ms >= created_at_ms)
);

-- Branch origin points only to an already-persisted message. It remains
-- nullable so deleting the source does not delete an independent branch.
CREATE TRIGGER chats_branch_message_must_exist_on_insert
BEFORE INSERT ON chats
WHEN NEW.branched_from_message_id IS NOT NULL
BEGIN
    SELECT CASE WHEN NOT EXISTS (
        SELECT 1 FROM messages WHERE id = NEW.branched_from_message_id
    ) THEN RAISE(ABORT, 'branched_from_message_id does not exist') END;
END;

CREATE TRIGGER chats_branch_message_must_exist_on_update
BEFORE UPDATE OF branched_from_message_id ON chats
WHEN NEW.branched_from_message_id IS NOT NULL
BEGIN
    SELECT CASE WHEN NOT EXISTS (
        SELECT 1 FROM messages WHERE id = NEW.branched_from_message_id
    ) THEN RAISE(ABORT, 'branched_from_message_id does not exist') END;
END;

CREATE TRIGGER clear_deleted_branch_message
AFTER DELETE ON messages
BEGIN
    UPDATE chats
       SET branched_from_message_id = NULL
     WHERE branched_from_message_id = OLD.id;
END;

-- A terminal message is historical evidence. Retry creates a new message.
CREATE TRIGGER messages_terminal_markdown_is_immutable
BEFORE UPDATE OF markdown ON messages
WHEN OLD.state <> 'pending' AND NEW.markdown <> OLD.markdown
BEGIN
    SELECT RAISE(ABORT, 'terminal message markdown is immutable');
END;

CREATE TRIGGER messages_terminal_state_is_absorbing
BEFORE UPDATE OF state ON messages
WHEN OLD.state IN ('complete', 'interrupted', 'failed') AND NEW.state <> OLD.state
BEGIN
    SELECT RAISE(ABORT, 'terminal message state is absorbing');
END;

-- Current document context for a chat. Unpinning affects future sends only;
-- message_document_refs preserves what an earlier request actually used.
CREATE TABLE chat_documents (
    chat_id            TEXT NOT NULL REFERENCES chats(id) ON DELETE CASCADE,
    document_id        TEXT NOT NULL,
    display_title      TEXT NOT NULL,
    pinned             INTEGER NOT NULL DEFAULT 1 CHECK (pinned IN (0, 1)),
    first_used_at_ms   INTEGER NOT NULL,
    last_used_at_ms    INTEGER NOT NULL,
    PRIMARY KEY (chat_id, document_id),
    CHECK (length(document_id) > 0),
    CHECK (last_used_at_ms >= first_used_at_ms)
);

-- Immutable provenance for one message. document_id is authority; title is a
-- user-content display snapshot and may be stale after rename. source_revision
-- is the opaque encoded Store revision used for context, never body content.
CREATE TABLE message_document_refs (
    message_id       TEXT NOT NULL REFERENCES messages(id) ON DELETE CASCADE,
    document_id      TEXT NOT NULL,
    relation         TEXT NOT NULL CHECK (relation IN ('context', 'exported_copy')),
    display_title    TEXT NOT NULL,
    source_revision  BLOB,
    used_at_ms       INTEGER NOT NULL,
    PRIMARY KEY (message_id, document_id, relation),
    CHECK (length(document_id) > 0),
    CHECK (
        (relation = 'context' AND source_revision IS NOT NULL) OR
        (relation = 'exported_copy' AND source_revision IS NULL)
    )
);

-- Finalized message Markdown is decomposed into stable search/deep-link units.
CREATE TABLE message_blocks (
    id              TEXT PRIMARY KEY,
    message_id      TEXT NOT NULL REFERENCES messages(id) ON DELETE CASCADE,
    ordinal         INTEGER NOT NULL CHECK (ordinal >= 0),
    kind            TEXT NOT NULL CHECK (
        kind IN ('paragraph', 'heading', 'list_item', 'quote', 'code', 'rule', 'other')
    ),
    markdown        TEXT NOT NULL,
    plain_text      TEXT NOT NULL,
    UNIQUE (message_id, ordinal),
    CHECK (length(id) > 0)
);

CREATE TRIGGER message_blocks_are_immutable
BEFORE UPDATE ON message_blocks
BEGIN
    SELECT RAISE(ABORT, 'message blocks are immutable');
END;

-- A disposable external-content FTS projection. Search is always ready locally;
-- the host-owned search.conf source choice controls federation, not this index.
CREATE VIRTUAL TABLE message_blocks_fts USING fts5(
    plain_text,
    content = 'message_blocks',
    content_rowid = 'rowid',
    tokenize = 'unicode61 remove_diacritics 2'
);

CREATE TRIGGER message_blocks_fts_insert
AFTER INSERT ON message_blocks
BEGIN
    INSERT INTO message_blocks_fts(rowid, plain_text)
    VALUES (NEW.rowid, NEW.plain_text);
END;

CREATE TRIGGER message_blocks_fts_delete
AFTER DELETE ON message_blocks
BEGIN
    INSERT INTO message_blocks_fts(message_blocks_fts, rowid, plain_text)
    VALUES ('delete', OLD.rowid, OLD.plain_text);
END;

-- One row per provider attempt, including failure/cancel. These rows power
-- /usage and later local reports. No prompt, response, title, document id,
-- message-block id, key, authorization value, or raw provider body is stored.
CREATE TABLE inference_attempts (
    id                          TEXT PRIMARY KEY,
    operation_id                TEXT NOT NULL UNIQUE,
    chat_id                     TEXT REFERENCES chats(id) ON DELETE SET NULL,
    assistant_message_id        TEXT REFERENCES messages(id) ON DELETE SET NULL,
    surface                     TEXT NOT NULL CHECK (surface IN ('standalone', 'context_pane')),
    intent                      TEXT NOT NULL DEFAULT 'chat' CHECK (intent = 'chat'),
    provider                    TEXT NOT NULL CHECK (length(provider) > 0),
    requested_model             TEXT NOT NULL CHECK (length(requested_model) > 0),
    resolved_model              TEXT,
    requested_reasoning         TEXT NOT NULL DEFAULT 'default' CHECK (
        requested_reasoning IN (
            'default', 'off', 'on', 'minimal', 'low', 'medium', 'high', 'xhigh', 'max'
        )
    ),
    effective_reasoning         TEXT CHECK (
        effective_reasoning IS NULL OR effective_reasoning IN (
            'default', 'off', 'on', 'minimal', 'low', 'medium', 'high', 'xhigh', 'max'
        )
    ),
    prompt_template_version     TEXT NOT NULL CHECK (length(prompt_template_version) > 0),
    status                      TEXT NOT NULL CHECK (
        status IN ('pending', 'streaming', 'complete', 'interrupted', 'failed', 'cancelled')
    ),
    queued_at_ms                INTEGER NOT NULL,
    context_started_at_ms       INTEGER,
    context_completed_at_ms     INTEGER,
    provider_started_at_ms      INTEGER,
    first_token_at_ms           INTEGER,
    completed_at_ms             INTEGER,
    request_payload_bytes       INTEGER CHECK (
        request_payload_bytes IS NULL OR request_payload_bytes >= 0
    ),
    context_payload_bytes       INTEGER NOT NULL DEFAULT 0 CHECK (context_payload_bytes >= 0),
    message_count               INTEGER NOT NULL DEFAULT 0 CHECK (message_count >= 0),
    context_document_count      INTEGER NOT NULL DEFAULT 0 CHECK (context_document_count >= 0),
    context_message_block_count INTEGER NOT NULL DEFAULT 0 CHECK (
        context_message_block_count >= 0
    ),
    prompt_tokens               INTEGER CHECK (prompt_tokens IS NULL OR prompt_tokens >= 0),
    cached_prompt_tokens        INTEGER CHECK (
        cached_prompt_tokens IS NULL OR cached_prompt_tokens >= 0
    ),
    completion_tokens           INTEGER CHECK (
        completion_tokens IS NULL OR completion_tokens >= 0
    ),
    reasoning_tokens            INTEGER CHECK (
        reasoning_tokens IS NULL OR reasoning_tokens >= 0
    ),
    total_tokens                INTEGER CHECK (total_tokens IS NULL OR total_tokens >= 0),
    usage_source                TEXT CHECK (
        usage_source IS NULL OR usage_source IN ('provider_reported', 'unavailable')
    ),
    stop_reason                 TEXT,
    error_class                 TEXT CHECK (
        error_class IS NULL OR error_class IN (
            'configuration', 'authentication', 'rate_limit', 'context_too_large',
            'model_unavailable', 'network', 'provider', 'cancelled', 'storage'
        )
    ),
    provider_response_id        TEXT,
    CHECK (length(id) > 0),
    CHECK (length(operation_id) > 0),
    CHECK (context_started_at_ms IS NULL OR context_started_at_ms >= queued_at_ms),
    CHECK (
        context_completed_at_ms IS NULL OR
        (context_started_at_ms IS NOT NULL AND context_completed_at_ms >= context_started_at_ms)
    ),
    CHECK (provider_started_at_ms IS NULL OR provider_started_at_ms >= queued_at_ms),
    CHECK (
        first_token_at_ms IS NULL OR
        (provider_started_at_ms IS NOT NULL AND first_token_at_ms >= provider_started_at_ms)
    ),
    CHECK (completed_at_ms IS NULL OR completed_at_ms >= queued_at_ms)
);

CREATE TRIGGER inference_attempt_terminal_state_is_absorbing
BEFORE UPDATE OF status ON inference_attempts
WHEN OLD.status IN ('complete', 'interrupted', 'failed', 'cancelled')
 AND NEW.status <> OLD.status
BEGIN
    SELECT RAISE(ABORT, 'terminal inference attempt state is absorbing');
END;

-- Feedback is append-only. Selecting Good, Bad, or clearing adds an event.
CREATE TABLE feedback_events (
    id              TEXT PRIMARY KEY,
    chat_id         TEXT REFERENCES chats(id) ON DELETE SET NULL,
    message_id      TEXT REFERENCES messages(id) ON DELETE SET NULL,
    attempt_id      TEXT REFERENCES inference_attempts(id) ON DELETE SET NULL,
    value           TEXT NOT NULL CHECK (value IN ('good', 'bad', 'cleared')),
    surface         TEXT NOT NULL CHECK (surface IN ('standalone', 'context_pane')),
    created_at_ms   INTEGER NOT NULL,
    CHECK (length(id) > 0)
);

-- Persisted before Store is called. This is the crash boundary that makes
-- Store-as-document retry-safe rather than merely session-idempotent.
CREATE TABLE document_exports (
    id                         TEXT PRIMARY KEY,
    idempotency_key            TEXT NOT NULL UNIQUE,
    chat_id                    TEXT NOT NULL REFERENCES chats(id) ON DELETE CASCADE,
    through_message_id         TEXT NOT NULL REFERENCES messages(id) ON DELETE CASCADE,
    destination_profile_id     TEXT NOT NULL,
    destination_workspace_id   TEXT NOT NULL,
    destination_parent_id      TEXT,
    destination_index          INTEGER CHECK (destination_index IS NULL OR destination_index >= 0),
    destination_title          TEXT NOT NULL,
    status                     TEXT NOT NULL CHECK (status IN ('pending', 'complete', 'failed')),
    document_id                TEXT,
    error_class                TEXT CHECK (
        error_class IS NULL OR error_class IN (
            'configuration', 'authentication', 'rate_limit', 'context_too_large',
            'model_unavailable', 'network', 'provider', 'cancelled', 'storage'
        )
    ),
    created_at_ms              INTEGER NOT NULL,
    completed_at_ms            INTEGER,
    CHECK (length(id) > 0),
    CHECK (length(idempotency_key) > 0),
    CHECK (length(destination_profile_id) > 0),
    CHECK (length(destination_workspace_id) > 0),
    CHECK (completed_at_ms IS NULL OR completed_at_ms >= created_at_ms),
    CHECK (status <> 'complete' OR document_id IS NOT NULL)
);

CREATE TRIGGER document_export_terminal_state_is_absorbing
BEFORE UPDATE OF status ON document_exports
WHEN OLD.status IN ('complete', 'failed') AND NEW.status <> OLD.status
BEGIN
    SELECT RAISE(ABORT, 'terminal document export state is absorbing');
END;

CREATE UNIQUE INDEX one_pending_assistant_per_chat
    ON messages(chat_id)
 WHERE state = 'pending' AND role = 'assistant';

CREATE INDEX chats_by_updated
    ON chats(updated_at_ms DESC);
CREATE INDEX messages_by_chat
    ON messages(chat_id, ordinal);
CREATE INDEX message_blocks_by_message
    ON message_blocks(message_id, ordinal);
CREATE INDEX chat_documents_by_document
    ON chat_documents(document_id, last_used_at_ms DESC);
CREATE INDEX message_document_refs_by_document
    ON message_document_refs(document_id, used_at_ms DESC);
CREATE INDEX inference_attempts_by_chat
    ON inference_attempts(chat_id, queued_at_ms DESC);
CREATE INDEX inference_attempts_by_completed
    ON inference_attempts(completed_at_ms DESC);
CREATE INDEX inference_attempts_by_dimensions
    ON inference_attempts(
        surface, provider, resolved_model, effective_reasoning, completed_at_ms DESC
    );
CREATE INDEX feedback_events_by_message
    ON feedback_events(message_id, created_at_ms DESC);
CREATE INDEX document_exports_by_chat
    ON document_exports(chat_id, created_at_ms DESC);

CREATE VIEW current_message_feedback AS
WITH ranked AS (
    SELECT
        id,
        chat_id,
        message_id,
        attempt_id,
        value,
        surface,
        created_at_ms,
        ROW_NUMBER() OVER (
            PARTITION BY CASE
                WHEN message_id IS NOT NULL THEN 'message:' || message_id
                ELSE 'attempt:' || attempt_id
            END
            ORDER BY created_at_ms DESC, id DESC
        ) AS rank
    FROM feedback_events
    WHERE message_id IS NOT NULL OR attempt_id IS NOT NULL
)
SELECT id, chat_id, message_id, attempt_id, value, surface, created_at_ms
FROM ranked
WHERE rank = 1 AND value <> 'cleared';

CREATE VIEW inference_attempt_durations AS
SELECT
    id,
    chat_id,
    assistant_message_id,
    surface,
    provider,
    requested_model,
    resolved_model,
    requested_reasoning,
    effective_reasoning,
    prompt_template_version,
    status,
    CASE
        WHEN context_started_at_ms IS NULL OR context_completed_at_ms IS NULL THEN NULL
        ELSE context_completed_at_ms - context_started_at_ms
    END AS context_assembly_ms,
    CASE
        WHEN provider_started_at_ms IS NULL THEN NULL
        ELSE provider_started_at_ms - queued_at_ms
    END AS provider_start_ms,
    CASE
        WHEN first_token_at_ms IS NULL OR provider_started_at_ms IS NULL THEN NULL
        ELSE first_token_at_ms - provider_started_at_ms
    END AS time_to_first_token_ms,
    CASE
        WHEN completed_at_ms IS NULL OR first_token_at_ms IS NULL THEN NULL
        ELSE completed_at_ms - first_token_at_ms
    END AS generation_ms,
    CASE
        WHEN completed_at_ms IS NULL THEN NULL
        ELSE completed_at_ms - queued_at_ms
    END AS total_ms,
    request_payload_bytes,
    context_payload_bytes,
    message_count,
    context_document_count,
    context_message_block_count,
    prompt_tokens,
    cached_prompt_tokens,
    completion_tokens,
    reasoning_tokens,
    total_tokens,
    usage_source,
    completed_at_ms
FROM inference_attempts;

CREATE VIEW usage_by_day AS
SELECT
    date(completed_at_ms / 1000, 'unixepoch') AS utc_day,
    surface,
    provider,
    COALESCE(resolved_model, requested_model) AS model,
    COALESCE(effective_reasoning, requested_reasoning) AS reasoning,
    prompt_template_version,
    status,
    COUNT(*) AS attempts,
    SUM(CASE WHEN usage_source = 'provider_reported' THEN 1 ELSE 0 END)
        AS attempts_with_reported_usage,
    SUM(request_payload_bytes) AS request_payload_bytes,
    SUM(context_payload_bytes) AS context_payload_bytes,
    SUM(prompt_tokens) AS prompt_tokens,
    SUM(cached_prompt_tokens) AS cached_prompt_tokens,
    SUM(completion_tokens) AS completion_tokens,
    SUM(reasoning_tokens) AS reasoning_tokens,
    SUM(total_tokens) AS total_tokens,
    AVG(CASE
        WHEN first_token_at_ms IS NULL OR provider_started_at_ms IS NULL THEN NULL
        ELSE first_token_at_ms - provider_started_at_ms
    END) AS average_time_to_first_token_ms,
    AVG(CASE
        WHEN completed_at_ms IS NULL THEN NULL
        ELSE completed_at_ms - queued_at_ms
    END) AS average_total_ms
FROM inference_attempts
WHERE completed_at_ms IS NOT NULL
GROUP BY utc_day, surface, provider, model, reasoning, prompt_template_version, status;

CREATE VIEW response_quality_by_model AS
SELECT
    attempt.provider,
    COALESCE(attempt.resolved_model, attempt.requested_model) AS model,
    COALESCE(attempt.effective_reasoning, attempt.requested_reasoning) AS reasoning,
    attempt.prompt_template_version,
    feedback.surface,
    COUNT(*) AS rated_responses,
    SUM(CASE WHEN feedback.value = 'good' THEN 1 ELSE 0 END) AS good_responses,
    SUM(CASE WHEN feedback.value = 'bad' THEN 1 ELSE 0 END) AS bad_responses
FROM current_message_feedback AS feedback
JOIN inference_attempts AS attempt ON attempt.id = feedback.attempt_id
GROUP BY
    attempt.provider,
    model,
    reasoning,
    attempt.prompt_template_version,
    feedback.surface;
