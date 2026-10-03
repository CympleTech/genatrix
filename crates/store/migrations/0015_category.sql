-- Where an item belongs (design 01, "分类"): the effective result of its
-- category annotations, cached for queries, the way `sensitivity` is.
-- A person's `category` is the user's word about a sender, applied to what
-- they send later.
ALTER TABLE item ADD COLUMN category TEXT NOT NULL DEFAULT 'personal';
ALTER TABLE person ADD COLUMN category TEXT;
CREATE INDEX item_quiet ON item(author, occurred_ms) WHERE category IN ('newsletter', 'promotion');
