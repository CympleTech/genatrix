-- Design 14: a workspace is a definition like a project, filed by hash.
--
-- SQLite cannot change a CHECK constraint in place, so the table is built
-- again with `workspace` among its kinds. Nothing references it.

CREATE TABLE definition_new (
    kind      TEXT NOT NULL CHECK (kind IN ('workspace', 'project', 'role', 'recipe')),
    name      TEXT NOT NULL,
    hash      TEXT NOT NULL,
    text      TEXT NOT NULL,
    added_ms  INTEGER NOT NULL,
    active    INTEGER NOT NULL DEFAULT 1,
    PRIMARY KEY (kind, hash)
);
INSERT INTO definition_new (kind, name, hash, text, added_ms, active)
    SELECT kind, name, hash, text, added_ms, active FROM definition;
DROP TABLE definition;
ALTER TABLE definition_new RENAME TO definition;
CREATE INDEX definition_name ON definition(kind, name, active);
