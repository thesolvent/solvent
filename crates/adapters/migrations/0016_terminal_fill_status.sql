CREATE TABLE inflight_fill_terminal (
    order_hash BLOB PRIMARY KEY NOT NULL CHECK(length(order_hash) = 32),
    status     TEXT NOT NULL CHECK(status IN ('confirmed', 'failed', 'dropped')),
    tx_hash    BLOB CHECK(tx_hash IS NULL OR length(tx_hash) = 32),
    block      INTEGER,
    reason     TEXT
);
