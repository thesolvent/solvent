-- The taker is promoted out of the saga body so a person's own orders can be listed and indexed,
-- the same way `state` is promoted for recovery scans.
ALTER TABLE crosschain_saga ADD COLUMN taker TEXT;

UPDATE crosschain_saga
SET taker = lower(json_extract(body, '$.taker'))
WHERE json_extract(body, '$.taker') IS NOT NULL;

CREATE INDEX crosschain_saga_taker ON crosschain_saga(taker, updated_at DESC);
