-- Shared trust schema 4 -> 5. No credential or sign-in continuation belongs here.
BEGIN IMMEDIATE;
ALTER TABLE enrollment_target RENAME TO enrollment_unattributed;
CREATE TABLE account_namespace (
    namespace TEXT PRIMARY KEY CHECK(length(namespace) = 64),
    active_account_id TEXT,
    next_record_version INTEGER NOT NULL DEFAULT 0 CHECK(next_record_version >= 0),
    legacy_retired INTEGER NOT NULL DEFAULT 0 CHECK(legacy_retired IN (0, 1)),
    FOREIGN KEY(namespace, active_account_id)
        REFERENCES account_directory(namespace, account_id) DEFERRABLE INITIALLY DEFERRED
);
CREATE TABLE account_directory (
    namespace TEXT NOT NULL REFERENCES account_namespace(namespace),
    account_id TEXT NOT NULL,
    auth_identity_id TEXT NOT NULL,
    state TEXT NOT NULL CHECK(state IN ('pending', 'ready')),
    record_version INTEGER NOT NULL CHECK(record_version > 0),
    PRIMARY KEY(namespace, account_id)
);
CREATE TABLE enrollment_targets (
    namespace TEXT NOT NULL,
    account_id TEXT NOT NULL,
    profile_id TEXT NOT NULL,
    PRIMARY KEY(namespace, account_id, profile_id),
    FOREIGN KEY(namespace, account_id)
        REFERENCES account_directory(namespace, account_id) ON DELETE CASCADE
);
PRAGMA user_version = 5;
COMMIT;
