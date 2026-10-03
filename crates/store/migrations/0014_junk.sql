-- Junk (design 01, "垃圾"): a person or a group or channel the user does
-- not want. What was there is removed; what arrives later is dropped before
-- it is stored. The mark is when it was set; NULL is not junk.
ALTER TABLE person ADD COLUMN junk_at TEXT;
ALTER TABLE thread ADD COLUMN junk_at TEXT;
