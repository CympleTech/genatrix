-- Genatrix ledger, schema version 1. See docs/design/08-storage.md.
-- One append-only table. State changes are new entries that reference the
-- earlier one; nothing here is ever updated or deleted.

CREATE TABLE entry (
    seq       INTEGER PRIMARY KEY,   -- contiguous from 1, assigned by the writer
    at        TEXT    NOT NULL,      -- RFC 3339 UTC
    kind      TEXT    NOT NULL,      -- egress, egress_result, run, run_step, action, ...
    subject   TEXT    NOT NULL,      -- id of the thing this entry is about
    body      TEXT    NOT NULL,      -- canonical JSON
    prev_hash BLOB    NOT NULL,      -- hash of entry seq-1; 32 zero bytes for seq 1
    hash      BLOB    NOT NULL       -- sha256 over (seq, prev_hash, at, kind, subject, body)
);
CREATE INDEX entry_subject ON entry(subject, seq);
CREATE INDEX entry_kind    ON entry(kind, seq);

CREATE TRIGGER entry_no_update BEFORE UPDATE ON entry BEGIN
    SELECT RAISE(ABORT, 'ledger is append-only');
END;
CREATE TRIGGER entry_no_delete BEFORE DELETE ON entry BEGIN
    SELECT RAISE(ABORT, 'ledger is append-only');
END;
