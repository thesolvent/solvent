-- The append-only Aqua event log and the per-chain resume cursor.
--
-- The event payload is the whole EventExt<AquaEvent> as JSONB, so replay is
-- lossless and reorg handling can be added later with no migration. The primary
-- key includes block_hash so a future reorg's replacement logs (same position,
-- different block) are distinct rows rather than silently deduplicated.

CREATE TABLE aqua_event (
    chain        BIGINT NOT NULL,
    block_number BIGINT NOT NULL,
    block_hash   BYTEA  NOT NULL,
    log_index    BIGINT NOT NULL,
    event        JSONB  NOT NULL,
    PRIMARY KEY (chain, block_number, block_hash, log_index)
);

CREATE INDEX aqua_event_replay ON aqua_event (chain, block_number, log_index);

CREATE TABLE registry_cursor (
    chain        BIGINT PRIMARY KEY,
    block_number BIGINT NOT NULL,
    log_index    BIGINT NOT NULL
);
