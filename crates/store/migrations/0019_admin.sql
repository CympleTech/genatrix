-- Design 06 v0.7, design 02 invariants 11 and 12: a paired device may be an
-- admin device, granted on this machine only, and high-privilege requests
-- from it carry the admin passphrase each time. The core keeps a slow hash
-- of the passphrase, never the passphrase, and counts wrong tries: at five,
-- remote administration is locked until this machine unlocks it.

ALTER TABLE device ADD COLUMN admin INTEGER NOT NULL DEFAULT 0;

CREATE TABLE admin (
    id          INTEGER PRIMARY KEY CHECK (id = 1),
    passphrase  TEXT,
    failures    INTEGER NOT NULL DEFAULT 0,
    locked_at   TEXT
);
INSERT INTO admin (id) VALUES (1);
