-- Design 15: interfaces. An interface is a set of static files the user
-- approved, kept in the data directory by the hash of its manifest; each
-- device picks one. The built-in default is not stored here: it is in the
-- core, and a device that picks nothing, or `default`, uses it.

CREATE TABLE ui (
    name        TEXT PRIMARY KEY,
    title       TEXT NOT NULL,
    -- The approved version in use; NULL until one is approved.
    current     TEXT,
    -- The device that made it, or `local`.
    made_by     TEXT NOT NULL,
    created_at  TEXT NOT NULL
);

CREATE TABLE ui_version (
    ui          TEXT NOT NULL REFERENCES ui(name),
    hash        TEXT NOT NULL,
    -- JSON: the interface API version it needs and each file's hash.
    manifest    TEXT NOT NULL,
    note        TEXT NOT NULL DEFAULT '',
    created_at  TEXT NOT NULL,
    approved_at TEXT,
    approved_by TEXT,
    PRIMARY KEY (ui, hash)
);

CREATE TABLE device_ui (
    -- A paired device's id, or `local` for this machine.
    device      TEXT PRIMARY KEY,
    ui          TEXT NOT NULL
);
