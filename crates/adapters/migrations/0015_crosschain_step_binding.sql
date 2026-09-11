ALTER TABLE crosschain_step
ADD COLUMN aggregate_id BLOB CHECK(aggregate_id IS NULL OR length(aggregate_id) = 32);

CREATE TABLE crosschain_order_binding (
    order_id      BLOB PRIMARY KEY NOT NULL CHECK(length(order_id) = 32),
    aggregate_id  BLOB UNIQUE NOT NULL CHECK(length(aggregate_id) = 32)
);
