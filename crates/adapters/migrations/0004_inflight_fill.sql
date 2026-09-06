-- Durable tracking of submitted, not-yet-terminal fills, so a restart recovers and reconciles them.
-- One row per in-flight fill; deleted once the fill settles. `handle_id` is the serialized walletkit
-- tracking handle, opaque here.
CREATE TABLE inflight_fill (
    order_hash  BLOB PRIMARY KEY,  -- the signed order hash (IntentId)
    reservation BLOB NOT NULL,     -- ReservationId whose holds this fill posts or voids
    handle_id   BLOB NOT NULL      -- serialized walletkit HandleId, for status lookups
);
