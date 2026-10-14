CREATE TABLE sync_state_events (
  id             INTEGER PRIMARY KEY AUTOINCREMENT,
  received_at    INTEGER NOT NULL,
  peer_device_id TEXT    NOT NULL,
  kind           TEXT    NOT NULL,
  profile_id     TEXT,
  workspace_id   TEXT,
  document_id    TEXT,
  document_name  TEXT,
  blob_hash      TEXT,
  detail         TEXT,
  items          INTEGER
);

CREATE INDEX sync_state_events_peer_id
ON sync_state_events(peer_device_id, id DESC);

CREATE INDEX sync_state_events_received_at
ON sync_state_events(received_at);
