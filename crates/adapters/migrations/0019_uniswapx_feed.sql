CREATE TABLE uniswapx_feed_order (
    order_hash                 BLOB    PRIMARY KEY,
    source_chain_id            INTEGER NOT NULL,
    token_in                   BLOB    NOT NULL,
    token_out                  BLOB    NOT NULL,
    amount_in                  TEXT    NOT NULL,
    required_out               TEXT    NOT NULL,
    market_out_per_in_q18      TEXT    NOT NULL,
    simulated_amount_out       TEXT    NOT NULL,
    simulated_batch_id         INTEGER NOT NULL,
    observed_at                INTEGER NOT NULL,
    last_seen_at               INTEGER NOT NULL
);

CREATE INDEX uniswapx_feed_order_seen_at ON uniswapx_feed_order (last_seen_at DESC);
