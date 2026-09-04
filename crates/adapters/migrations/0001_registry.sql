-- The append-only Aqua event log and the per-chain resume cursor.
--
-- The event payload is the whole EventExt<AquaEvent> as JSON text, so replay is
-- lossless and reorg handling can be added later with no migration. The primary
-- key includes block_hash so a future reorg's replacement logs (same position,
-- different block) are distinct rows rather than silently deduplicated.

CREATE TABLE aqua_event (
    chain        INTEGER NOT NULL,
    block_number INTEGER NOT NULL,
    block_hash   BLOB    NOT NULL,
    log_index    INTEGER NOT NULL,
    event        TEXT    NOT NULL,
    PRIMARY KEY (chain, block_number, block_hash, log_index)
);

CREATE INDEX aqua_event_replay ON aqua_event (chain, block_number, log_index);

CREATE TABLE registry_cursor (
    chain        INTEGER PRIMARY KEY,
    block_number INTEGER NOT NULL,
    log_index    INTEGER NOT NULL
);
