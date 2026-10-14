-- Agent identity schema 3 -> 4: exact unfinished automatic-enrollment target.
BEGIN IMMEDIATE;
CREATE TABLE enrollment_target (
    singleton INTEGER PRIMARY KEY CHECK(singleton = 1),
    profile_id TEXT NOT NULL
);
PRAGMA user_version = 4;
COMMIT;