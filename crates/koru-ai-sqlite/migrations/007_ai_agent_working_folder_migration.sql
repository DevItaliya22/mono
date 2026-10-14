-- AI schema 6 -> 7: the folder a chat's local agent works in (AGENTUX I8).

-- Null means the person's home folder, which every agent accepts and which
-- every chat used before this column existed. A chosen folder is an
-- absolute path on this machine; AI history never enters sync, so the path
-- names nothing on another device.
ALTER TABLE chats ADD COLUMN agent_working_folder TEXT
    CHECK (agent_working_folder IS NULL OR length(agent_working_folder) > 0);
