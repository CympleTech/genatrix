-- The text read out of attachments by the core's reader (design 11), kept
-- so a file is read once. `reader` names the reader's version: text read
-- by an older one is read again. A file that could not be read keeps why,
-- so it is not tried on every run.
CREATE TABLE blob_text (
    hash        TEXT PRIMARY KEY,
    reader      TEXT NOT NULL,
    text        TEXT,
    error       TEXT,
    read_at     TEXT NOT NULL
);
