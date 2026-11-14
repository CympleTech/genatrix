-- Deleting a conversation (design 01 v0.7, "删除对话"): what was there is
-- removed, and what arrives later is kept. The time is when it was
-- deleted: a message dated before it that comes again, from a history
-- backfill or a refetch, is not brought back. NULL is never deleted.
ALTER TABLE person ADD COLUMN cleared_at TEXT;
ALTER TABLE thread ADD COLUMN cleared_at TEXT;
