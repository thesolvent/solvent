-- What sourcing the delivery would have cost, gas included, in the input token — recorded whether
-- or not the trade filled. Routing computes it before it consults the taker's bound, and a decline
-- previously discarded it, leaving a declined trade with no legs and no account of itself. With it
-- the shortfall is `indicative_amount_in - amount_in`, which separates "priced out by a hair" from
-- "no liquidity at all". Nullable: no candidate had capacity, so there was no cost to quote.
ALTER TABLE trade ADD COLUMN indicative_amount_in TEXT;
