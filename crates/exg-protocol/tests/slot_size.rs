use exg_common::{
    Decimal128, MarginMode, OrderId, OrderType, Side, SymbolId, TimeInForce, UnixMicros, UserId,
};
use exg_protocol::Command;

const SLOT_SIZE: usize = 4096;

fn dec(s: &str) -> Decimal128 {
    s.parse().unwrap()
}

fn ts() -> UnixMicros {
    UnixMicros::from_micros(1_700_000_000_000_000)
}

fn maximal_new_order() -> Command {
    Command::NewOrder {
        order_id: OrderId::new(u64::MAX),
        user_id: UserId::new(u64::MAX),
        symbol: SymbolId::new(u16::MAX),
        side: Side::Buy,
        order_type: OrderType::Iceberg,
        time_in_force: TimeInForce::Gtd,
        price: Some(dec("999999999999.999999999999999999")),
        quantity: dec("999999999999.999999999999999999"),
        stop_price: Some(dec("999999999999.999999999999999999")),
        trailing_delta: Some(dec("999999999.999999")),
        visible_quantity: Some(dec("100.0")),
        reduce_only: true,
        margin_mode: MarginMode::Cross,
        leverage: Some(dec("125")),
        client_order_id: Some(u64::MAX),
        timestamp: ts(),
    }
}

fn maximal_cancel() -> Command {
    Command::CancelOrder {
        order_id: OrderId::new(u64::MAX),
        user_id: UserId::new(u64::MAX),
        symbol: SymbolId::new(u16::MAX),
        timestamp: ts(),
    }
}

fn maximal_amend() -> Command {
    Command::AmendOrder {
        order_id: OrderId::new(u64::MAX),
        user_id: UserId::new(u64::MAX),
        symbol: SymbolId::new(u16::MAX),
        new_price: Some(dec("999999999999.999999999999999999")),
        new_quantity: Some(dec("999999999999.999999999999999999")),
        timestamp: ts(),
    }
}

fn maximal_cancel_all() -> Command {
    Command::CancelAllOrders {
        user_id: UserId::new(u64::MAX),
        symbol: SymbolId::new(u16::MAX),
        timestamp: ts(),
    }
}

fn check(name: &str, cmd: Command) {
    let bytes = rkyv::to_bytes::<rkyv::rancor::Error>(&cmd)
        .unwrap_or_else(|e| panic!("rkyv encode {name}: {e}"));
    assert!(
        bytes.len() <= SLOT_SIZE,
        "{name}: rkyv encoded size {} exceeds ring buffer slot_size {}",
        bytes.len(),
        SLOT_SIZE
    );
}

#[test]
fn maximal_new_order_fits_in_slot() {
    check("NewOrder", maximal_new_order());
}

#[test]
fn maximal_cancel_fits_in_slot() {
    check("CancelOrder", maximal_cancel());
}

#[test]
fn maximal_amend_fits_in_slot() {
    check("AmendOrder", maximal_amend());
}

#[test]
fn maximal_cancel_all_fits_in_slot() {
    check("CancelAllOrders", maximal_cancel_all());
}
