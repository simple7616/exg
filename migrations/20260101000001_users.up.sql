-- Users and authentication
CREATE TABLE users (
    user_id BIGINT PRIMARY KEY,
    email VARCHAR(255) NOT NULL UNIQUE,
    password_hash VARCHAR(255) NOT NULL,
    totp_secret VARCHAR(64),
    kyc_level SMALLINT NOT NULL DEFAULT 0,
    is_active BOOLEAN NOT NULL DEFAULT true,
    created_at BIGINT NOT NULL,  -- UnixMicros
    updated_at BIGINT NOT NULL
);

CREATE INDEX idx_users_email ON users(email);

CREATE TABLE api_keys (
    key_id VARCHAR(64) PRIMARY KEY,
    user_id BIGINT NOT NULL REFERENCES users(user_id),
    secret_key VARCHAR(255) NOT NULL,
    label VARCHAR(128) NOT NULL DEFAULT '',
    can_trade BOOLEAN NOT NULL DEFAULT true,
    can_withdraw BOOLEAN NOT NULL DEFAULT false,
    can_read BOOLEAN NOT NULL DEFAULT true,
    is_active BOOLEAN NOT NULL DEFAULT true,
    created_at BIGINT NOT NULL,
    ip_whitelist TEXT[] NOT NULL DEFAULT '{}'
);

CREATE INDEX idx_api_keys_user ON api_keys(user_id);

CREATE TABLE sub_accounts (
    account_id BIGINT PRIMARY KEY,
    user_id BIGINT NOT NULL REFERENCES users(user_id),
    label VARCHAR(128) NOT NULL,
    is_active BOOLEAN NOT NULL DEFAULT true,
    created_at BIGINT NOT NULL
);

CREATE INDEX idx_sub_accounts_user ON sub_accounts(user_id);

CREATE TABLE login_history (
    id BIGSERIAL PRIMARY KEY,
    user_id BIGINT NOT NULL REFERENCES users(user_id),
    ip_address VARCHAR(45) NOT NULL,
    user_agent TEXT,
    success BOOLEAN NOT NULL,
    created_at BIGINT NOT NULL
);

CREATE INDEX idx_login_history_user ON login_history(user_id, created_at DESC);
