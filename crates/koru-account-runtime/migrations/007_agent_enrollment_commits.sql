-- Agent identity schema 2 -> 3: one-use attested enrollment evidence.
BEGIN IMMEDIATE;
CREATE TABLE enrollment_commits (
    attestation_jti TEXT PRIMARY KEY,
    peer_device_id TEXT NOT NULL,
    prepared_json BLOB NOT NULL
);
PRAGMA user_version = 3;
COMMIT;
