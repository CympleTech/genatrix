-- What the user says about a relationship (design 07): roles they confirmed,
-- notes they wrote. The statistics are computed from items, not stored.
CREATE TABLE relationship (
    person_id  TEXT PRIMARY KEY REFERENCES person(id),
    roles      TEXT NOT NULL DEFAULT '[]',   -- JSON list of labels
    notes      TEXT NOT NULL DEFAULT '',
    updated_at TEXT NOT NULL
);
CREATE INDEX item_author_time ON item(author, occurred_ms);
