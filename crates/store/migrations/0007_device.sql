-- Paired devices (design 06, "形态"; design 08, network boundary): the
-- browsers allowed to reach the interface from another address. The core
-- keeps only a hash of each device's secret. A revoked device stays, with
-- its revocation time, so the list says what happened.
CREATE TABLE device (
    id           TEXT PRIMARY KEY,
    name         TEXT NOT NULL,
    secret_hash  TEXT NOT NULL,
    created_at   TEXT NOT NULL,
    last_seen    TEXT,
    revoked_at   TEXT
);
