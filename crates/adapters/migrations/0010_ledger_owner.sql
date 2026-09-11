-- Rebate restoration and user swaps compete for the same inventory, but their lifecycle owners
-- differ. Rebate expiry is coordinated with the strategy guard; swap expiry settles an intent.

DROP INDEX ledger_reservation_state;
ALTER TABLE ledger_reservation RENAME TO ledger_reservation_legacy;

CREATE TABLE ledger_reservation (
    id           BLOB PRIMARY KEY,
    owner_kind   TEXT    NOT NULL CHECK (owner_kind IN ('swap', 'rebate')),
    owner_id     BLOB    NOT NULL,
    sources      TEXT    NOT NULL,
    state        TEXT    NOT NULL,
    filled       TEXT,
    expires_at   INTEGER NOT NULL
);

INSERT INTO ledger_reservation (id, owner_kind, owner_id, sources, state, filled, expires_at)
SELECT id, 'swap', intent, sources, state, filled, expires_at
FROM ledger_reservation_legacy;

DROP TABLE ledger_reservation_legacy;
CREATE INDEX ledger_reservation_state ON ledger_reservation (state);
