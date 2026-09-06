-- The trade lifecycle: one `trade` per submitted swap, its stage timeline (`trade_attempt`), and
-- the makers it sourced (`trade_leg`). The signed UniswapX order lives as columns on `trade`, not a
-- separate table. Ids are stored as their raw bytes (`BLOB`); U256 amounts as decimal `TEXT`
-- (lossless); the trade id is the ULID text (time-sortable, so `ORDER BY id` is the newest-first
-- page order). `status_rank` mirrors the status's lifecycle rank so a transition can be guarded
-- forward-only in one statement.

CREATE TABLE trade (
    id                TEXT    PRIMARY KEY,
    order_hash        BLOB    NOT NULL,
    taker             BLOB    NOT NULL,
    token_in          BLOB    NOT NULL,
    token_out         BLOB    NOT NULL,
    amount_in         TEXT    NOT NULL,
    min_amount_out    TEXT    NOT NULL,
    amount_out        TEXT,
    status            TEXT    NOT NULL,
    status_rank       INTEGER NOT NULL,
    deadline_block    INTEGER NOT NULL,
    signature         BLOB,
    price_impact_pct  REAL,
    surplus           TEXT,
    tx_hash           BLOB,
    block_number      INTEGER,
    created_at        INTEGER NOT NULL,
    settled_at        INTEGER
);

-- One trade per signed order: a resubmit conflicts here and is deduped to the existing row.
CREATE UNIQUE INDEX trade_order_hash ON trade (order_hash);
CREATE INDEX trade_status ON trade (status);
CREATE INDEX trade_taker ON trade (taker);

CREATE TABLE trade_attempt (
    trade_id  TEXT    NOT NULL REFERENCES trade (id),
    status    TEXT    NOT NULL,
    at        INTEGER NOT NULL,
    PRIMARY KEY (trade_id, status)
);

-- A leg holds only leg-specific data: which maker, which strategy, and its slice of the split. The
-- tokens and the settlement tx are the parent trade's (one pair, one fill tx), so they are read from
-- `trade`, not repeated here.
CREATE TABLE trade_leg (
    trade_id      TEXT    NOT NULL REFERENCES trade (id),
    idx           INTEGER NOT NULL,
    maker         BLOB    NOT NULL,
    strategy_hash BLOB    NOT NULL,
    amount_in     TEXT    NOT NULL,
    amount_out    TEXT    NOT NULL,
    PRIMARY KEY (trade_id, idx)
);
