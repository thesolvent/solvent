-- Token USD prices captured at submit, so a trade's fee and value figures stay at trade-time
-- economics instead of being re-valued at the current market. Nullable: a trade on an unpriced token
-- simply has no stored price (the read side renders the fee as unavailable).
ALTER TABLE trade ADD COLUMN token_in_price_usd  REAL;
ALTER TABLE trade ADD COLUMN token_out_price_usd REAL;
