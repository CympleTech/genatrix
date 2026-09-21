-- Where each connector has got to in each subdivision of each account.
-- The cursor itself is the connector's business and is stored as it hands
-- it over; the store only keys it.
CREATE TABLE sync_cursor (
    connector   TEXT NOT NULL,
    account     TEXT NOT NULL,
    scope       TEXT NOT NULL,
    cursor      TEXT NOT NULL,
    updated_at  TEXT NOT NULL,
    PRIMARY KEY (connector, account, scope)
);
