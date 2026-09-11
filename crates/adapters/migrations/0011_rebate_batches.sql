-- One open batch per protected strategy. Its state payload includes any prepared or public
-- authorization, while accrual rows retain attribution independently for later settlement.

CREATE TABLE rebate_batch (
    id             BLOB PRIMARY KEY,
    maker          BLOB    NOT NULL,
    app            BLOB    NOT NULL,
    strategy_hash  BLOB    NOT NULL,
    token_lo       BLOB    NOT NULL,
    token_hi       BLOB    NOT NULL,
    state          TEXT    NOT NULL CHECK (
        state IN ('accumulating', 'guarded', 'preparing', 'ready', 'invalidating', 'closed')
    ),
    state_payload  TEXT,
    is_open        INTEGER NOT NULL CHECK (is_open IN (0, 1))
);

CREATE UNIQUE INDEX rebate_batch_open_strategy
ON rebate_batch (maker, app, strategy_hash)
WHERE is_open = 1;

CREATE TABLE rebate_accrual (
    batch_id        BLOB NOT NULL,
    maker           BLOB NOT NULL,
    app             BLOB NOT NULL,
    strategy_hash   BLOB NOT NULL,
    trade_id        TEXT NOT NULL,
    payload         TEXT NOT NULL,
    PRIMARY KEY (batch_id, trade_id),
    UNIQUE (maker, app, strategy_hash, trade_id),
    FOREIGN KEY (batch_id) REFERENCES rebate_batch(id)
);

CREATE INDEX rebate_accrual_batch ON rebate_accrual (batch_id);
