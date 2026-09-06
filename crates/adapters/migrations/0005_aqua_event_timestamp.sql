-- The event log has no on-chain timestamp, so the store stamps observation time (unix seconds) at
-- insert. This backs the activity feed's `at` and the stats window count (`events_24h`). Existing
-- rows (devnet only) default to 0.
ALTER TABLE aqua_event ADD COLUMN created_at INTEGER NOT NULL DEFAULT 0;

CREATE INDEX aqua_event_recorded ON aqua_event (chain, created_at);
