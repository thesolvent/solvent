-- The durable reservation index: the current lifecycle state of every reservation
-- the ledger has admitted. `sources` and `filled` are JSON text (lossless, no
-- migration when the shapes grow). The primary key is the reservation id
-- (`hash(intentId ‖ routePlanHash)`), so a duplicate reserve is a no-op. A restart
-- rebuilds the in-memory holds by replaying the rows still in state 'pending'.

CREATE TABLE ledger_reservation (
    id           BLOB PRIMARY KEY,
    intent       BLOB    NOT NULL,
    sources      TEXT    NOT NULL,
    state        TEXT    NOT NULL,
    filled       TEXT,
    expires_at   INTEGER NOT NULL
);

CREATE INDEX ledger_reservation_state ON ledger_reservation (state);
