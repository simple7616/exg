-- TimescaleDB hypertables for time-series data
-- Note: Requires TimescaleDB extension

CREATE EXTENSION IF NOT EXISTS timescaledb;

CREATE TABLE trades_ts (
    time TIMESTAMPTZ NOT NULL,
    symbol_id SMALLINT NOT NULL,
    trade_id BIGINT NOT NULL,
    price NUMERIC(38,18) NOT NULL,
    qty NUMERIC(38,18) NOT NULL,
    side VARCHAR(4) NOT NULL,
    buyer_order_id BIGINT,
    seller_order_id BIGINT
);

SELECT create_hypertable('trades_ts', 'time');
CREATE INDEX idx_trades_ts_symbol ON trades_ts(symbol_id, time DESC);

CREATE TABLE klines_1m (
    time TIMESTAMPTZ NOT NULL,
    symbol_id SMALLINT NOT NULL,
    open NUMERIC(38,18) NOT NULL,
    high NUMERIC(38,18) NOT NULL,
    low NUMERIC(38,18) NOT NULL,
    close NUMERIC(38,18) NOT NULL,
    volume NUMERIC(38,18) NOT NULL,
    quote_volume NUMERIC(38,18) NOT NULL,
    trade_count BIGINT NOT NULL DEFAULT 0
);

SELECT create_hypertable('klines_1m', 'time');
CREATE UNIQUE INDEX idx_klines_1m ON klines_1m(symbol_id, time);

-- Retention policies (optional, configure based on needs)
-- SELECT add_retention_policy('trades_ts', INTERVAL '90 days');
-- SELECT add_retention_policy('klines_1m', INTERVAL '365 days');
