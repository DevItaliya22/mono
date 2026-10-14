-- Device-local catalog visibility only. The Profile row remains because deleted
-- Workspace tombstones retain it as their owner and recovery must remain possible.
ALTER TABLE profiles ADD COLUMN removed_at INTEGER;
