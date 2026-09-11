CREATE TABLE crosschain_leg_quote (
    quote_id    BLOB PRIMARY KEY NOT NULL CHECK(length(quote_id) = 32),
    expires_at  INTEGER NOT NULL,
    body        TEXT NOT NULL
);

CREATE TABLE crosschain_preparation (
    token       BLOB PRIMARY KEY NOT NULL CHECK(length(token) = 32),
    quote_id    BLOB NOT NULL CHECK(length(quote_id) = 32),
    role        TEXT NOT NULL CHECK(role IN ('origin', 'destination')),
    chain_id    INTEGER NOT NULL,
    state       TEXT NOT NULL CHECK(state IN ('prepared', 'committed', 'executed', 'released')),
    expires_at  INTEGER NOT NULL,
    body        TEXT NOT NULL,
    UNIQUE(quote_id, role)
);

CREATE TABLE crosschain_saga (
    order_id    BLOB PRIMARY KEY NOT NULL CHECK(length(order_id) = 32),
    state       TEXT NOT NULL,
    body        TEXT NOT NULL,
    updated_at  INTEGER NOT NULL DEFAULT (unixepoch())
);

CREATE INDEX crosschain_saga_recovery ON crosschain_saga(state, updated_at);

CREATE TABLE crosschain_step (
    order_id    BLOB NOT NULL CHECK(length(order_id) = 32),
    command     TEXT NOT NULL,
    body        TEXT NOT NULL,
    PRIMARY KEY(order_id, command)
);
