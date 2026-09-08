-- AquaEvent is externally tagged, so the strategy hash lives under its variant.
-- Keep strategy replay indexed without changing the persisted event payload.
CREATE INDEX aqua_event_strategy_history ON aqua_event (
    chain,
    COALESCE(
        json_extract(event, '$.event.Shipped.strategy_hash'),
        json_extract(event, '$.event.Pushed.strategy_hash'),
        json_extract(event, '$.event.Pulled.strategy_hash'),
        json_extract(event, '$.event.Docked.strategy_hash')
    ),
    block_number,
    log_index
);
