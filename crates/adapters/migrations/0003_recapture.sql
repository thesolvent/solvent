-- Per-maker recapture credits a confirmed fill earned, keyed by (intent, maker, token) so a
-- re-driven reconcile re-accrues the same fill exactly once (INSERT OR IGNORE). `settled` flips
-- when the payout worker has rebated the credit. `amount` is the decimal base-unit string
-- (lossless — no i64 range limit).

CREATE TABLE recapture_credit (
    intent   BLOB    NOT NULL,
    maker    BLOB    NOT NULL,
    token    BLOB    NOT NULL,
    amount   TEXT    NOT NULL,
    settled  INTEGER NOT NULL DEFAULT 0,
    PRIMARY KEY (intent, maker, token)
);

CREATE INDEX recapture_credit_settled ON recapture_credit (settled);
