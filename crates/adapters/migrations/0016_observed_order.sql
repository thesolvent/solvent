-- Every order the feed showed us, whatever became of it. The trade table only holds orders this
-- resolver acted on, so the ones refused at the door — a native-currency leg, a token we make no
-- market in — left no trace, and there was no denominator to measure reach against.
CREATE TABLE IF NOT EXISTS observed_order (
    order_hash   BLOB PRIMARY KEY,
    source       TEXT    NOT NULL,
    chain        INTEGER NOT NULL,
    token_in     BLOB    NOT NULL,
    token_out    BLOB,               -- absent when the order has no single output token
    amount_in    TEXT    NOT NULL,
    required_out TEXT,               -- what the settler demands, priced at `seen_at`
    verdict      TEXT    NOT NULL,   -- admitted | dropped
    reason       TEXT,               -- the admission rule that refused it
    trade_id     TEXT,               -- set once an admitted order becomes a trade
    seen_at      INTEGER NOT NULL
);
CREATE INDEX IF NOT EXISTS observed_order_seen_at ON observed_order (seen_at DESC);
