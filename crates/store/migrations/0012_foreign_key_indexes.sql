-- Indexes for the child side of two foreign keys. Deleting an annotation
-- or a raw record makes SQLite look for rows that point at it; without an
-- index each look is a scan of the whole table, and removing a connector's
-- data (tens of thousands of rows) turns into billions of comparisons.
CREATE INDEX annotation_superseded_by ON annotation(superseded_by);
CREATE INDEX item_raw ON item(raw_id);
