-- Commitments (design 07) and daily digests (design 03, 06).
CREATE TABLE commitment (
    id           TEXT PRIMARY KEY,
    from_person  TEXT NOT NULL REFERENCES person(id),
    to_person    TEXT REFERENCES person(id),
    what         TEXT NOT NULL,
    due          TEXT,
    evidence     TEXT NOT NULL,            -- JSON list of item ids
    status       TEXT NOT NULL,
    standing     TEXT NOT NULL,
    created_at   TEXT NOT NULL
);
CREATE INDEX commitment_from ON commitment(from_person, status);

CREATE TABLE digest (
    day          TEXT PRIMARY KEY,         -- YYYY-MM-DD, local
    generated_at TEXT NOT NULL,
    value        TEXT NOT NULL             -- JSON of the whole Digest
);
