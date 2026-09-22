-- Actions (design 03): the whole action as JSON, with the columns the
-- interface and the connectors query by. The store does not read the
-- JSON; the agent layer owns the type.
CREATE TABLE action (
    id          TEXT PRIMARY KEY,
    run_id      TEXT NOT NULL,
    kind        TEXT NOT NULL,
    account     TEXT,
    status      TEXT NOT NULL,
    created_at  TEXT NOT NULL,
    expires_at  TEXT NOT NULL,
    value       TEXT NOT NULL
);
CREATE INDEX action_status ON action(status, created_at);
CREATE INDEX action_account ON action(account, status);
