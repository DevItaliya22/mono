-- Device-local irreversible X request evidence, <root>/agent/socials-delivery.db.
-- Never replicated. Text parts are the frozen payload, never credentials.
CREATE TABLE social_attempts (
    workspace BLOB NOT NULL CHECK(length(workspace)=16),
    post BLOB NOT NULL CHECK(length(post)=16),
    channel BLOB NOT NULL CHECK(length(channel)=16),
    attempt BLOB NOT NULL CHECK(length(attempt)=16),
    device BLOB NOT NULL CHECK(length(device)=16),
    actor TEXT NOT NULL,
    phase TEXT NOT NULL CHECK(phase IN ('ready','sending','published','failed','needs_connection','unconfirmed','cancelled')),
    next_at_ms INTEGER,
    failures INTEGER NOT NULL CHECK(failures>=0),
    request BLOB CHECK(request IS NULL OR length(request)=16),
    error TEXT,
    PRIMARY KEY(workspace,post,channel)
);
CREATE TABLE social_parts (
    workspace BLOB NOT NULL,
    post BLOB NOT NULL,
    channel BLOB NOT NULL,
    part INTEGER NOT NULL CHECK(part>=0),
    text TEXT NOT NULL,
    remote_id TEXT,
    remote_url TEXT,
    published_at_ms INTEGER,
    PRIMARY KEY(workspace,post,channel,part),
    FOREIGN KEY(workspace,post,channel) REFERENCES social_attempts(workspace,post,channel),
    CHECK((remote_id IS NULL AND remote_url IS NULL AND published_at_ms IS NULL)
       OR (remote_id IS NOT NULL AND published_at_ms IS NOT NULL))
);
PRAGMA user_version=1;
