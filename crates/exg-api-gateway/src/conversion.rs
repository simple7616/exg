use exg_common::{
    Decimal128, MarginMode, OrderId, OrderType, Side, SymbolId, TimeInForce, UnixMicros, UserId,
};
use exg_protocol::Command;

use crate::error::ApiError;
use crate::types::{AmendOrderRequest, CancelOrderRequest, PlaceOrderRequest};

// ── Side conversion ──────────────────────────────────────────────────────

pub fn side_to_string(side: Side) -> &'static str {
    match side {
        Side::Buy => "BUY",
        Side::Sell => "SELL",
    }
}

pub fn string_to_side(s: &str) -> Result<Side, ApiError> {
    match s {
        "BUY" => Ok(Side::Buy),
        "SELL" => Ok(Side::Sell),
        _ => Err(ApiError::bad_request(format!("Invalid side: {s}"))),
    }
}

// ── OrderType conversion ─────────────────────────────────────────────────

pub fn order_type_to_string(ot: OrderType) -> &'static str {
    match ot {
        OrderType::Limit => "LIMIT",
        OrderType::Market => "MARKET",
        OrderType::StopMarket => "STOP_MARKET",
        OrderType::StopLimit => "STOP_LIMIT",
        OrderType::TakeProfitMarket => "TAKE_PROFIT_MARKET",
        OrderType::TakeProfitLimit => "TAKE_PROFIT_LIMIT",
        OrderType::TrailingStop => "TRAILING_STOP",
        OrderType::Iceberg => "ICEBERG",
    }
}

pub fn string_to_order_type(s: &str) -> Result<OrderType, ApiError> {
    match s {
        "LIMIT" => Ok(OrderType::Limit),
        "MARKET" => Ok(OrderType::Market),
        "STOP_MARKET" => Ok(OrderType::StopMarket),
        "STOP_LIMIT" => Ok(OrderType::StopLimit),
        "TAKE_PROFIT_MARKET" => Ok(OrderType::TakeProfitMarket),
        "TAKE_PROFIT_LIMIT" => Ok(OrderType::TakeProfitLimit),
        "TRAILING_STOP" => Ok(OrderType::TrailingStop),
        "ICEBERG" => Ok(OrderType::Iceberg),
        _ => Err(ApiError::bad_request(format!("Invalid order type: {s}"))),
    }
}

// ── TimeInForce conversion ───────────────────────────────────────────────

pub fn tif_to_string(tif: TimeInForce) -> &'static str {
    match tif {
        TimeInForce::Gtc => "GTC",
        TimeInForce::Ioc => "IOC",
        TimeInForce::Fok => "FOK",
        TimeInForce::Gtd => "GTD",
        TimeInForce::PostOnly => "POST_ONLY",
    }
}

pub fn string_to_tif(s: &str) -> Result<TimeInForce, ApiError> {
    match s {
        "GTC" => Ok(TimeInForce::Gtc),
        "IOC" => Ok(TimeInForce::Ioc),
        "FOK" => Ok(TimeInForce::Fok),
        "GTD" => Ok(TimeInForce::Gtd),
        "POST_ONLY" => Ok(TimeInForce::PostOnly),
        _ => Err(ApiError::bad_request(format!("Invalid time in force: {s}"))),
    }
}

// ── PlaceOrderRequest -> Command::NewOrder ───────────────────────────────

fn parse_decimal(s: &str, field: &str) -> Result<Decimal128, ApiError> {
    s.parse::<Decimal128>()
        .map_err(|_| ApiError::bad_request(format!("Invalid {field}: {s}")))
}

/// Convert a `PlaceOrderRequest` into a `Command::NewOrder`.
///
/// Validates:
/// - Side and order type strings
/// - Limit orders must have a price
/// - Quantity must be a valid decimal
pub fn to_new_order_command(
    req: &PlaceOrderRequest,
    user_id: UserId,
    symbol: SymbolId,
    order_id: OrderId,
    timestamp: UnixMicros,
) -> Result<Command, ApiError> {
    let side = string_to_side(&req.side)?;
    let order_type = string_to_order_type(&req.order_type)?;
    let quantity = parse_decimal(&req.quantity, "quantity")?;

    let time_in_force = match &req.time_in_force {
        Some(tif) => string_to_tif(tif)?,
        None => match order_type {
            OrderType::Market => TimeInForce::Ioc,
            _ => TimeInForce::Gtc,
        },
    };

    let price = req
        .price
        .as_deref()
        .map(|p| parse_decimal(p, "price"))
        .transpose()?;

    // Limit-type orders require a price.
    if order_type.is_limit() && price.is_none() {
        return Err(ApiError::bad_request("Limit order requires a price"));
    }

    let stop_price = req
        .stop_price
        .as_deref()
        .map(|p| parse_decimal(p, "stop_price"))
        .transpose()?;

    let client_order_id = req
        .client_order_id
        .as_deref()
        .map(|s| {
            s.parse::<u64>()
                .map_err(|_| ApiError::bad_request(format!("Invalid client_order_id: {s}")))
        })
        .transpose()?;

    Ok(Command::NewOrder {
        order_id,
        user_id,
        symbol,
        side,
        order_type,
        time_in_force,
        price,
        quantity,
        stop_price,
        trailing_delta: None,
        visible_quantity: None,
        reduce_only: req.reduce_only.unwrap_or(false),
        margin_mode: MarginMode::Cross,
        leverage: None,
        client_order_id,
        timestamp,
    })
}

// ── CancelOrderRequest -> Command::CancelOrder ───────────────────────────

pub fn to_cancel_order_command(
    req: &CancelOrderRequest,
    user_id: UserId,
    symbol: SymbolId,
    ts: UnixMicros,
) -> Result<Command, ApiError> {
    Ok(Command::CancelOrder {
        order_id: OrderId::new(req.order_id),
        user_id,
        symbol,
        timestamp: ts,
    })
}

// ── AmendOrderRequest -> Command::AmendOrder ─────────────────────────────

pub fn to_amend_order_command(
    req: &AmendOrderRequest,
    user_id: UserId,
    symbol: SymbolId,
    ts: UnixMicros,
) -> Result<Command, ApiError> {
    if req.new_price.is_none() && req.new_quantity.is_none() {
        return Err(ApiError::bad_request(
            "amend: at least one of newPrice or newQuantity must be present",
        ));
    }
    let new_price = req
        .new_price
        .as_deref()
        .map(|s| s.parse::<Decimal128>())
        .transpose()
        .map_err(|e| ApiError::bad_request(format!("newPrice: {e}")))?;
    let new_quantity = req
        .new_quantity
        .as_deref()
        .map(|s| s.parse::<Decimal128>())
        .transpose()
        .map_err(|e| ApiError::bad_request(format!("newQuantity: {e}")))?;
    Ok(Command::AmendOrder {
        order_id: OrderId::new(req.order_id),
        user_id,
        symbol,
        new_price,
        new_quantity,
        timestamp: ts,
    })
}
