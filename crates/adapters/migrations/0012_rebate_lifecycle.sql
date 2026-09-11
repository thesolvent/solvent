-- Rebate log progress is independent from Aqua registry progress. A settlement marker is written
-- before the ledger reservation is posted so restart recovery can finish either side safely.

CREATE TABLE rebate_scan_cursor (
    chain        INTEGER PRIMARY KEY,
    block_number INTEGER NOT NULL
);

CREATE TABLE rebate_settlement (
    batch_id BLOB PRIMARY KEY REFERENCES rebate_batch (id),
    status   TEXT NOT NULL CHECK (status IN ('settling', 'executed')),
    payload  TEXT NOT NULL
);

CREATE INDEX rebate_settlement_status ON rebate_settlement (status);
