-- Upgrade existing X evidence without changing attempt IDs or confirmed parts.
ALTER TABLE social_attempts ADD COLUMN network TEXT NOT NULL DEFAULT 'x'
    CHECK(network IN ('x','linkedin_person','linkedin_page','reddit'));
ALTER TABLE social_attempts ADD COLUMN subreddit TEXT;
ALTER TABLE social_attempts ADD COLUMN reddit_title TEXT;
ALTER TABLE social_attempts ADD COLUMN flair_id TEXT;
PRAGMA user_version=2;
