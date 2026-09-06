-- Quotes served, for maker analytics (uptime, latency, fill-share). One row per /swap/quote in
-- `quote_event`; one row per sourced strategy in `quote_participant`. A no-route quote records the
-- event with no participants (it still counts toward a pair's quote total).

CREATE TABLE quote_event (
    id         TEXT    NOT NULL PRIMARY KEY,
    chain_id   INTEGER NOT NULL,
    pair_lo    BLOB    NOT NULL,
    pair_hi    BLOB    NOT NULL,
    served_at  INTEGER NOT NULL,
    latency_ms INTEGER NOT NULL
);

CREATE INDEX quote_event_pair ON quote_event (chain_id, pair_lo, pair_hi, served_at);

CREATE TABLE quote_participant (
    quote_id      TEXT NOT NULL REFERENCES quote_event (id),
    maker         BLOB NOT NULL,
    strategy_hash BLOB NOT NULL,
    PRIMARY KEY (quote_id, strategy_hash)
);

CREATE INDEX quote_participant_maker ON quote_participant (maker);
CREATE INDEX quote_participant_strategy ON quote_participant (strategy_hash);
