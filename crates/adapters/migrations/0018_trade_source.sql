-- Where the order reached us. `ProtocolId` selects the decoder and cannot answer this: the public
-- UniswapX book and this resolver's own submit endpoint both carry V2 Dutch orders, cosigned by
-- different keys and competed for on entirely different terms. Existing rows predate the feed, so
-- they are the self-venue path.
ALTER TABLE trade ADD COLUMN source TEXT NOT NULL DEFAULT 'solvent';
