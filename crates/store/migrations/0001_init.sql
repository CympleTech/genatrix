-- Genatrix main database, schema version 1.
-- Entities follow docs/design/01-data-model.md; storage rules follow 08.
-- Nested values (payload, producer, member lists) are JSON text. New kinds
-- are new variants plus a migration, never a free-form column.

CREATE TABLE raw (
    id           TEXT PRIMARY KEY,
    connector    TEXT NOT NULL,
    account      TEXT NOT NULL,
    external_id  TEXT NOT NULL,
    fetched_at   TEXT NOT NULL,
    content_type TEXT NOT NULL,
    hash         TEXT NOT NULL,
    size         INTEGER NOT NULL,
    UNIQUE (connector, account, external_id, hash)
);

CREATE TABLE thread (
    id          TEXT PRIMARY KEY,
    kind        TEXT NOT NULL,
    connector   TEXT NOT NULL,
    account     TEXT NOT NULL,
    external_id TEXT NOT NULL,
    title       TEXT,
    members     TEXT NOT NULL DEFAULT '[]',
    first_at    TEXT,
    last_at     TEXT,
    UNIQUE (connector, account, external_id)
);

CREATE TABLE person (
    id           TEXT PRIMARY KEY,
    display_name TEXT NOT NULL,
    is_self      INTEGER NOT NULL DEFAULT 0,
    merged_from  TEXT NOT NULL DEFAULT '[]'
);

CREATE TABLE handle (
    id         TEXT PRIMARY KEY,
    person_id  TEXT NOT NULL REFERENCES person(id),
    kind       TEXT NOT NULL,
    value      TEXT NOT NULL,
    confidence TEXT NOT NULL,
    UNIQUE (kind, value)
);
CREATE INDEX handle_person ON handle(person_id);

CREATE TABLE blob (
    hash      TEXT PRIMARY KEY,
    mime      TEXT NOT NULL,
    size      INTEGER NOT NULL,
    name_hint TEXT
);

CREATE TABLE item (
    id             TEXT PRIMARY KEY,
    connector      TEXT NOT NULL,
    account        TEXT NOT NULL,
    external_id    TEXT NOT NULL,
    raw_id         TEXT NOT NULL REFERENCES raw(id),
    supersedes     TEXT REFERENCES item(id),
    thread_id      TEXT NOT NULL REFERENCES thread(id),
    occurred_at    TEXT NOT NULL,      -- RFC 3339 with the source's offset
    occurred_ms    INTEGER NOT NULL,   -- same instant in UTC milliseconds, for ordering
    ingested_at    TEXT NOT NULL,
    direction      TEXT NOT NULL,
    author         TEXT REFERENCES person(id),
    recipients     TEXT NOT NULL DEFAULT '[]',
    text           TEXT NOT NULL,
    blobs          TEXT NOT NULL DEFAULT '[]',
    sensitivity    TEXT NOT NULL DEFAULT 'personal',
    tombstoned     INTEGER NOT NULL DEFAULT 0,
    kind           TEXT NOT NULL,
    payload        TEXT NOT NULL,
    UNIQUE (connector, account, external_id, raw_id)
);
CREATE INDEX item_timeline ON item(occurred_ms DESC);
CREATE INDEX item_thread   ON item(thread_id, occurred_ms);
CREATE INDEX item_source   ON item(connector, account, external_id);
CREATE INDEX item_author   ON item(author);
CREATE INDEX item_supersedes ON item(supersedes);

-- Full-text index over the normalized text. Trigram tokenizer so CJK
-- substring search works without a segmenter (design 08, phase one).
CREATE VIRTUAL TABLE item_fts USING fts5(
    text,
    content='item',
    content_rowid='rowid',
    tokenize='trigram'
);
CREATE TRIGGER item_fts_ai AFTER INSERT ON item BEGIN
    INSERT INTO item_fts(rowid, text) VALUES (new.rowid, new.text);
END;
CREATE TRIGGER item_fts_ad AFTER DELETE ON item BEGIN
    INSERT INTO item_fts(item_fts, rowid, text) VALUES ('delete', old.rowid, old.text);
END;
CREATE TRIGGER item_fts_au AFTER UPDATE OF text ON item BEGIN
    INSERT INTO item_fts(item_fts, rowid, text) VALUES ('delete', old.rowid, old.text);
    INSERT INTO item_fts(rowid, text) VALUES (new.rowid, new.text);
END;

CREATE TABLE annotation (
    id            TEXT PRIMARY KEY,
    item_id       TEXT NOT NULL REFERENCES item(id),
    producer      TEXT NOT NULL,   -- JSON
    created_at    TEXT NOT NULL,
    kind          TEXT NOT NULL,   -- discriminator, duplicated from the JSON for indexing
    value         TEXT NOT NULL,   -- JSON of the whole AnnotationKind
    superseded_by TEXT REFERENCES annotation(id)
);
CREATE INDEX annotation_item ON annotation(item_id, kind);
