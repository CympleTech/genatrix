-- Executors (design 13): the definitions work is drafted from, and the
-- record of each dispatch. What an executor said is kept here, by step,
-- not as items: it is not part of the user's life to search or digest, and
-- it is untrusted text, shown at the dispatch's level and never used as
-- evidence.

-- Projects, roles and recipes, by hash. A changed definition is a new row;
-- the old one stays, because a dispatch names the hash it was approved with.
CREATE TABLE definition (
    kind      TEXT NOT NULL CHECK (kind IN ('project', 'role', 'recipe')),
    name      TEXT NOT NULL,
    hash      TEXT NOT NULL,
    text      TEXT NOT NULL,
    added_ms  INTEGER NOT NULL,
    active    INTEGER NOT NULL DEFAULT 1,
    PRIMARY KEY (kind, hash)
);
CREATE INDEX definition_name ON definition(kind, name, active);

CREATE TABLE dispatch (
    -- The id of the approved Dispatch action.
    id            TEXT PRIMARY KEY,
    project       TEXT NOT NULL,
    recipe        TEXT NOT NULL,
    recipe_hash   TEXT NOT NULL,
    state         TEXT NOT NULL CHECK (state IN ('running', 'finished', 'stopped', 'failed')),
    level         TEXT NOT NULL,
    branch        TEXT NOT NULL,
    base          TEXT,
    tip           TEXT,
    review_passed INTEGER,
    detail        TEXT,
    started_ms    INTEGER NOT NULL,
    ended_ms      INTEGER
);
CREATE INDEX dispatch_started ON dispatch(started_ms DESC);

CREATE TABLE dispatch_step (
    dispatch_id TEXT NOT NULL REFERENCES dispatch(id) ON DELETE CASCADE,
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
    PRIMARY KEY (dispatch_id, seq)
);

CREATE TABLE dispatch_event (
    dispatch_id TEXT NOT NULL REFERENCES dispatch(id) ON DELETE CASCADE,
    seq         INTEGER NOT NULL,
    n           INTEGER NOT NULL,
    at_ms       INTEGER NOT NULL,
    kind        TEXT NOT NULL,
    text        TEXT NOT NULL,
    PRIMARY KEY (dispatch_id, seq, n)
);
