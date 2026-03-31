-- Trading symbols and margin tiers
CREATE TABLE symbols (
    symbol_id SMALLINT PRIMARY KEY,
    name VARCHAR(32) NOT NULL UNIQUE,
    base_asset VARCHAR(16) NOT NULL,
    quote_asset VARCHAR(16) NOT NULL,
    symbol_type VARCHAR(32) NOT NULL,  -- 'perpetual_linear', 'perpetual_inverse', 'spot'
    status VARCHAR(32) NOT NULL DEFAULT 'trading',
    tick_size NUMERIC(38,18) NOT NULL,
    lot_size NUMERIC(38,18) NOT NULL,
    min_notional NUMERIC(38,18) NOT NULL,
    max_leverage NUMERIC(38,18) NOT NULL DEFAULT 125,
    maker_fee NUMERIC(38,18) NOT NULL DEFAULT 0.0002,
    taker_fee NUMERIC(38,18) NOT NULL DEFAULT 0.0005,
    created_at BIGINT NOT NULL,
    updated_at BIGINT NOT NULL
);

CREATE TABLE margin_tiers (
    id SERIAL PRIMARY KEY,
    symbol_id SMALLINT NOT NULL REFERENCES symbols(symbol_id),
    tier_level SMALLINT NOT NULL,
    notional_floor NUMERIC(38,18) NOT NULL,
    notional_cap NUMERIC(38,18) NOT NULL,
    maintenance_margin_rate NUMERIC(38,18) NOT NULL,
    maintenance_amount NUMERIC(38,18) NOT NULL,
    UNIQUE(symbol_id, tier_level)
);

CREATE INDEX idx_margin_tiers_symbol ON margin_tiers(symbol_id, tier_level);
