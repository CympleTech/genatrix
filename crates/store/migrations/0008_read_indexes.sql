-- Indexes for the three questions the interface asks most often, each of
-- which was a full scan.
--
-- The People page groups every item by who wrote it, and wants the newest
-- and the connectors per person. Without these that is two passes over the
-- whole item table plus a temporary b-tree for every person; with them both
-- passes are answered from the index and never touch the table. Partial, on
-- the live rows only, because that is the only set the page ever asks about
-- and it keeps the index small.
CREATE INDEX item_author_live ON item(author, occurred_ms)
    WHERE tombstoned = 0 AND author IS NOT NULL;
CREATE INDEX item_author_conn ON item(author, connector)
    WHERE tombstoned = 0 AND author IS NOT NULL;

-- The status line counts the items with a summary and the items with a
-- judgement. `annotation_item` is (item_id, kind), which cannot answer "how
-- many of this kind" without reading every annotation there is; this one
-- can, from the index alone.
CREATE INDEX annotation_kind ON annotation(kind, item_id)
    WHERE superseded_by IS NULL;
