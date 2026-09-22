-- Chunks of item text and their vectors (design 08: sqlite-vec, in the
-- same file). The vector itself is also on the Embedding annotation, as
-- design 01 records it; this table is the index over those.
CREATE TABLE chunk (
    id        INTEGER PRIMARY KEY,
    item_id   TEXT NOT NULL REFERENCES item(id),
    idx       INTEGER NOT NULL,
    start     INTEGER NOT NULL,
    end_      INTEGER NOT NULL,
    UNIQUE (item_id, idx)
);
CREATE INDEX chunk_item ON chunk(item_id);

-- 384 dimensions: multilingual-e5-small. A different embedder is a new
-- migration and a re-embedding, never a silent mismatch.
CREATE VIRTUAL TABLE chunk_vec USING vec0(embedding float[384]);
