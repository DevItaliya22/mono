-- AI schema 7 -> 8: a local agent's own words on a tool row (AGENTUX, the
-- CTO's decision of 2026-09-25 after the third look: "let's do the schema
-- update so we can improve the UX"; UC17 on "add so I can review").

-- Null on every row Koru's own tools wrote and on every row written before
-- this schema: such a row reads by its kind and outcome, as before. A local
-- agent's row keeps the agent's label for the call, what a permission
-- request asked, what the call was given and what it produced — each
-- bounded by the worker before it is written (the label and summary to
-- LIVE_TEXT_LIMIT bytes, the input and output to AGENT_CONTENT_CHARS
-- characters), so the checks here are the ceiling, not the rule.
ALTER TABLE tool_invocations ADD COLUMN agent_label TEXT
    CHECK (agent_label IS NULL OR length(agent_label) BETWEEN 1 AND 1000);
ALTER TABLE tool_invocations ADD COLUMN agent_summary TEXT
    CHECK (agent_summary IS NULL OR length(agent_summary) BETWEEN 1 AND 1000);
ALTER TABLE tool_invocations ADD COLUMN agent_input TEXT
    CHECK (agent_input IS NULL OR length(agent_input) BETWEEN 1 AND 4000);
ALTER TABLE tool_invocations ADD COLUMN agent_output TEXT
    CHECK (agent_output IS NULL OR length(agent_output) BETWEEN 1 AND 4000);
