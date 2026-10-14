ALTER TABLE inference_attempts
    ADD COLUMN web_search_requested INTEGER NOT NULL DEFAULT 0 CHECK (web_search_requested IN (0, 1));
ALTER TABLE inference_attempts
    ADD COLUMN web_search_calls INTEGER NOT NULL DEFAULT 0 CHECK (web_search_calls >= 0);

CREATE TABLE message_citations (
    message_id TEXT NOT NULL REFERENCES messages(id) ON DELETE CASCADE,
    ordinal INTEGER NOT NULL CHECK (ordinal >= 0),
    url TEXT NOT NULL,
    title TEXT NOT NULL,
    PRIMARY KEY (message_id, ordinal),
    UNIQUE (message_id, url),
    CHECK (length(url) > 0)
);
