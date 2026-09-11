-- Materialized sort and filter keys keep the public history feed off JSON scans while the signed
-- settlement payload remains the canonical record.
ALTER TABLE rebate_settlement ADD COLUMN maker BLOB;
ALTER TABLE rebate_settlement ADD COLUMN executed_at INTEGER;

UPDATE rebate_settlement
SET maker = (
        SELECT maker FROM rebate_batch WHERE rebate_batch.id = rebate_settlement.batch_id
    ),
    executed_at = CAST(json_extract(payload, '$.executed_at') AS INTEGER);

CREATE INDEX rebate_settlement_history
ON rebate_settlement (status, executed_at DESC, batch_id DESC);

CREATE INDEX rebate_settlement_maker_history
ON rebate_settlement (maker, status, executed_at DESC, batch_id DESC);
