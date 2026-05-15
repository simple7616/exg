-- Stage 1a §9 invariant 13: per-user client_order_id dedup table
CREATE TABLE user_client_order_ids (
    user_id BIGINT NOT NULL,
    client_order_id BIGINT NOT NULL,
    created_at BIGINT NOT NULL,  -- UnixMicros
    PRIMARY KEY (user_id, client_order_id)
);
CREATE INDEX idx_user_client_order_ids_created_at
    ON user_client_order_ids (created_at);
