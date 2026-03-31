-- Deposits and withdrawals
CREATE TABLE deposits (
    id BIGSERIAL PRIMARY KEY,
    user_id BIGINT NOT NULL REFERENCES users(user_id),
    chain VARCHAR(32) NOT NULL,
    tx_hash VARCHAR(128) NOT NULL,
    log_index INT NOT NULL DEFAULT 0,
    from_address VARCHAR(128) NOT NULL,
    to_address VARCHAR(128) NOT NULL,
    amount NUMERIC(38,18) NOT NULL,
    asset VARCHAR(16) NOT NULL,
    confirmations INT NOT NULL DEFAULT 0,
    required_confirmations INT NOT NULL,
    status VARCHAR(32) NOT NULL DEFAULT 'PENDING',  -- PENDING, CONFIRMED, CREDITED
    created_at BIGINT NOT NULL,
    updated_at BIGINT NOT NULL,
    UNIQUE(chain, tx_hash, log_index)
);

CREATE INDEX idx_deposits_user ON deposits(user_id, created_at DESC);
CREATE INDEX idx_deposits_status ON deposits(status) WHERE status = 'PENDING';

CREATE TABLE withdrawals (
    id BIGSERIAL PRIMARY KEY,
    user_id BIGINT NOT NULL REFERENCES users(user_id),
    chain VARCHAR(32) NOT NULL,
    to_address VARCHAR(128) NOT NULL,
    amount NUMERIC(38,18) NOT NULL,
    fee NUMERIC(38,18) NOT NULL DEFAULT 0,
    asset VARCHAR(16) NOT NULL,
    tx_hash VARCHAR(128),
    status VARCHAR(32) NOT NULL DEFAULT 'PENDING_REVIEW',
    -- PENDING_REVIEW, APPROVED, PROCESSING, COMPLETED, REJECTED, FAILED
    reviewed_by BIGINT,
    created_at BIGINT NOT NULL,
    updated_at BIGINT NOT NULL
);

CREATE INDEX idx_withdrawals_user ON withdrawals(user_id, created_at DESC);
CREATE INDEX idx_withdrawals_status ON withdrawals(status) WHERE status IN ('PENDING_REVIEW', 'APPROVED', 'PROCESSING');
