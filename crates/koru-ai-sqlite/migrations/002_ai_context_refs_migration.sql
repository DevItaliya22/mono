-- AI schema 1 -> 2: immutable provenance for message-block context.

CREATE TABLE message_block_refs (
    message_id       TEXT NOT NULL REFERENCES messages(id) ON DELETE CASCADE,
    source_block_id  TEXT NOT NULL REFERENCES message_blocks(id) ON DELETE CASCADE,
    used_at_ms       INTEGER NOT NULL,
    PRIMARY KEY (message_id, source_block_id)
);

CREATE INDEX message_block_refs_by_source
    ON message_block_refs(source_block_id, used_at_ms DESC);
