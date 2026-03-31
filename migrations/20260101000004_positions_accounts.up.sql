-- Positions and accounts
CREATE TABLE positions (
    user_id BIGINT NOT NULL,
    symbol_id SMALLINT NOT NULL,
    side VARCHAR(8) NOT NULL,           -- 'LONG', 'SHORT', 'BOTH'
    size NUMERIC(38,18) NOT NULL DEFAULT 0,
    entry_price NUMERIC(38,18) NOT NULL DEFAULT 0,
    leverage NUMERIC(38,18) NOT NULL DEFAULT 1,
    margin NUMERIC(38,18) NOT NULL DEFAULT 0,
    unrealized_pnl NUMERIC(38,18) NOT NULL DEFAULT 0,
    accumulated_funding NUMERIC(38,18) NOT NULL DEFAULT 0,
    margin_mode VARCHAR(16) NOT NULL DEFAULT 'CROSS',
    updated_at BIGINT NOT NULL,
    PRIMARY KEY (user_id, symbol_id)
);

CREATE TABLE accounts (
    user_id BIGINT NOT NULL,
    wallet_type VARCHAR(32) NOT NULL,   -- 'SPOT', 'FUTURES', 'FUNDING', etc.
    available NUMERIC(38,18) NOT NULL DEFAULT 0,
    frozen NUMERIC(38,18) NOT NULL DEFAULT 0,
    margin NUMERIC(38,18) NOT NULL DEFAULT 0,
    updated_at BIGINT NOT NULL,
    PRIMARY KEY (user_id, wallet_type)
);

CREATE TABLE journal_entries (
    id BIGSERIAL PRIMARY KEY,
    debit_user_id BIGINT NOT NULL,
    debit_wallet VARCHAR(32) NOT NULL,
    debit_field VARCHAR(16) NOT NULL,   -- 'available', 'frozen', 'margin'
    credit_user_id BIGINT NOT NULL,
    credit_wallet VARCHAR(32) NOT NULL,
    credit_field VARCHAR(16) NOT NULL,
    amount NUMERIC(38,18) NOT NULL,
    entry_type VARCHAR(32) NOT NULL,
    idempotency_key VARCHAR(128) NOT NULL UNIQUE,
    created_at BIGINT NOT NULL
);

CREATE INDEX idx_journal_debit_user ON journal_entries(debit_user_id, created_at DESC);
CREATE INDEX idx_journal_credit_user ON journal_entries(credit_user_id, created_at DESC);
CREATE INDEX idx_journal_idempotency ON journal_entries(idempotency_key);
