CREATE INDEX trade_leg_strategy_trade ON trade_leg (strategy_hash, trade_id);
CREATE INDEX trade_leg_maker_trade ON trade_leg (maker, trade_id);
