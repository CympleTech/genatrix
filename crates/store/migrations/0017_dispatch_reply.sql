-- Design 13 v0.3, "等你回答": a step may end with a question for the user,
-- and the work waits for the answer.
--
-- A dispatch gains two states: `waiting` for an answer, and `answered`
-- until the core carries the work on. A step keeps the user's answer apart
-- from what the executor said, with the level it leaves at: `public` for
-- one of the executor's own options or words the user marked public,
-- `personal` otherwise.
--
-- SQLite cannot change a CHECK constraint in place, so the three tables
-- are built again. The order matters: foreign keys are on, and dropping a
-- table that is still referenced would delete the rows that reference it.
-- The new children point at the new parent; the old tables are dropped
-- children first, when nothing points at the old parent any more; the
-- renames then carry the references along.

CREATE TABLE dispatch_new (
    -- The id of the approved Dispatch action.
    id            TEXT PRIMARY KEY,
    project       TEXT NOT NULL,
    recipe        TEXT NOT NULL,
    recipe_hash   TEXT NOT NULL,
    state         TEXT NOT NULL CHECK (state IN
                      ('running', 'waiting', 'answered', 'finished', 'stopped', 'failed')),
    level         TEXT NOT NULL,
    branch        TEXT NOT NULL,
    base          TEXT,
    tip           TEXT,
    review_passed INTEGER,
    detail        TEXT,
    started_ms    INTEGER NOT NULL,
    ended_ms      INTEGER
);
INSERT INTO dispatch_new SELECT * FROM dispatch;

CREATE TABLE dispatch_step_new (
    dispatch_id TEXT NOT NULL REFERENCES dispatch_new(id) ON DELETE CASCADE,
    seq         INTEGER NOT NULL,
    step        TEXT NOT NULL,
    round       INTEGER NOT NULL,
    role        TEXT NOT NULL,
    executor    TEXT NOT NULL,
    state       TEXT NOT NULL,
    egress_id   TEXT,
    stop        TEXT,
    answer      TEXT,
    verdict     TEXT,
    commit_id   TEXT,
    error       TEXT,
    started_ms  INTEGER NOT NULL,
    ended_ms    INTEGER,
    reply       TEXT,
    reply_level TEXT,
    PRIMARY KEY (dispatch_id, seq)
);
INSERT INTO dispatch_step_new
    (dispatch_id, seq, step, round, role, executor, state, egress_id, stop, answer, verdict,
     commit_id, error, started_ms, ended_ms)
SELECT dispatch_id, seq, step, round, role, executor, state, egress_id, stop, answer, verdict,
       commit_id, error, started_ms, ended_ms
FROM dispatch_step;

CREATE TABLE dispatch_event_new (
    dispatch_id TEXT NOT NULL REFERENCES dispatch_new(id) ON DELETE CASCADE,
    seq         INTEGER NOT NULL,
    n           INTEGER NOT NULL,
    at_ms       INTEGER NOT NULL,
    kind        TEXT NOT NULL,
    text        TEXT NOT NULL,
    PRIMARY KEY (dispatch_id, seq, n)
);
INSERT INTO dispatch_event_new SELECT * FROM dispatch_event;

DROP TABLE dispatch_event;
DROP TABLE dispatch_step;
DROP TABLE dispatch;

ALTER TABLE dispatch_new RENAME TO dispatch;
ALTER TABLE dispatch_step_new RENAME TO dispatch_step;
ALTER TABLE dispatch_event_new RENAME TO dispatch_event;
CREATE INDEX dispatch_started ON dispatch(started_ms DESC);
