-- AI schema 3 -> 4: per-chat visibility on agent and MCP read surfaces.

ALTER TABLE chats ADD COLUMN private INTEGER NOT NULL DEFAULT 0
    CHECK (private IN (0, 1));
