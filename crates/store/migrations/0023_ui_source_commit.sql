-- Design 15 v0.4: the commit of an interface's source a version was built
-- from, when an executor built it; the source follows it, so the next
-- change starts where the last one ended.
ALTER TABLE ui_version ADD COLUMN source_commit TEXT;
