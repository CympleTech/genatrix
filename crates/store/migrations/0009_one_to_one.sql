-- A person's conversation is now only what passed one to one (design 06
-- v0.6): every person query leaves out items in group and channel threads.
-- That condition reads thread_id, so the two indexes the People and Chats
-- pages lean on carry it too, and the scans stay inside the index.
DROP INDEX item_author_live;
CREATE INDEX item_author_live ON item(author, occurred_ms, thread_id)
    WHERE tombstoned = 0 AND author IS NOT NULL;
DROP INDEX item_author_conn;
CREATE INDEX item_author_conn ON item(author, connector, thread_id)
    WHERE tombstoned = 0 AND author IS NOT NULL;
