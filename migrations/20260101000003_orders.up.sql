-- Orders and trades
CREATE TABLE orders (
    order_id BIGINT PRIMARY KEY,
    user_id BIGINT NOT NULL,
    symbol_id SMALLINT NOT NULL,
    client_order_id BIGINT,
    side VARCHAR(4) NOT NULL,           -- 'BUY', 'SELL'
    order_type VARCHAR(32) NOT NULL,    -- 'LIMIT', 'MARKET', etc.
    time_in_force VARCHAR(16),
    price NUMERIC(38,18),
    stop_price NUMERIC(38,18),
    original_qty NUMERIC(38,18) NOT NULL,
    executed_qty NUMERIC(38,18) NOT NULL DEFAULT 0,
    remaining_qty NUMERIC(38,18) NOT NULL,
    status VARCHAR(32) NOT NULL,        -- 'NEW', 'PARTIALLY_FILLED', 'FILLED', etc.
    margin_mode VARCHAR(16) NOT NULL DEFAULT 'CROSS',
    leverage NUMERIC(38,18),
    reduce_only BOOLEAN NOT NULL DEFAULT false,
    avg_fill_price NUMERIC(38,18) NOT NULL DEFAULT 0,
    commission NUMERIC(38,18) NOT NULL DEFAULT 0,
    created_at BIGINT NOT NULL,
    updated_at BIGINT NOT NULL
);

CREATE INDEX idx_orders_user_status ON orders(user_id, status) WHERE status IN ('NEW', 'PARTIALLY_FILLED', 'PENDING_TRIGGER');
CREATE INDEX idx_orders_user_symbol ON orders(user_id, symbol_id, created_at DESC);
CREATE INDEX idx_orders_client_id ON orders(user_id, client_order_id) WHERE client_order_id IS NOT NULL;

CREATE TABLE trades (
    trade_id BIGINT PRIMARY KEY,
    symbol_id SMALLINT NOT NULL,
    price NUMERIC(38,18) NOT NULL,
    qty NUMERIC(38,18) NOT NULL,
    buyer_order_id BIGINT NOT NULL,
    seller_order_id BIGINT NOT NULL,
    buyer_user_id BIGINT NOT NULL,
    seller_user_id BIGINT NOT NULL,
    buyer_fee NUMERIC(38,18) NOT NULL,
    seller_fee NUMERIC(38,18) NOT NULL,
    buyer_is_maker BOOLEAN NOT NULL,
    created_at BIGINT NOT NULL
);

CREATE INDEX idx_trades_symbol ON trades(symbol_id, created_at DESC);
CREATE INDEX idx_trades_buyer ON trades(buyer_user_id, created_at DESC);
CREATE INDEX idx_trades_seller ON trades(seller_user_id, created_at DESC);
