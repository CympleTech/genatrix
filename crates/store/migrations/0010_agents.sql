-- Functional agents (design 11): who is installed, which versions the
-- user approved, and what each run did. An agent's own data is not here:
-- each has its own encrypted space file beside this database.

CREATE TABLE agent (
    id           TEXT PRIMARY KEY,
    name         TEXT NOT NULL,
    -- The hash of the approved package that runs now.
    version      TEXT NOT NULL,
    state        TEXT NOT NULL CHECK (state IN ('active', 'paused')),
    -- The highest level of anything the agent has read; its space is
    -- derived data at this level (design 02, propagation).
    space_level  TEXT NOT NULL DEFAULT 'public',
    installed_ms INTEGER NOT NULL
);

-- Every version the user approved, so a rollback needs no new approval.
CREATE TABLE agent_version (
    agent_id    TEXT NOT NULL REFERENCES agent(id) ON DELETE CASCADE,
    hash        TEXT NOT NULL,
    manifest    TEXT NOT NULL,
    approved_ms INTEGER NOT NULL,
    PRIMARY KEY (agent_id, hash)
);

-- One row per run. The log and the answer are the agent's words and may
-- carry what it read, so the row has the run's level and is shown by it.
CREATE TABLE agent_run (
    id         TEXT PRIMARY KEY,
    agent_id   TEXT NOT NULL REFERENCES agent(id) ON DELETE CASCADE,
    version    TEXT NOT NULL,
    started_ms INTEGER NOT NULL,
    invocation TEXT NOT NULL,
    input      TEXT,
    outcome    TEXT NOT NULL,
    detail     TEXT,
    answer     TEXT,
    level      TEXT NOT NULL,
    reads      TEXT NOT NULL DEFAULT '[]',
    proposals  TEXT NOT NULL DEFAULT '[]',
    log        TEXT NOT NULL DEFAULT '[]',
    fuel       INTEGER NOT NULL
);
CREATE INDEX agent_run_agent ON agent_run(agent_id, started_ms DESC);
