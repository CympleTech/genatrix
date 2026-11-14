-- Design 11 v0.2, design 15: an agent's views. The latest under each name,
-- the run's level, and when; data of a fixed shape, never interface code.
CREATE TABLE agent_view (
    agent   TEXT NOT NULL,
    name    TEXT NOT NULL,
    title   TEXT NOT NULL,
    -- JSON, as the core wrote it from the agent's view record.
    data    TEXT NOT NULL,
    level   TEXT NOT NULL,
    at      TEXT NOT NULL,
    PRIMARY KEY (agent, name)
);
