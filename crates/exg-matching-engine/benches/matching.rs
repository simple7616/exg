use criterion::{BatchSize, Criterion, Throughput, black_box, criterion_group, criterion_main};

use exg_common::{
    Decimal128, MarginMode, OrderId, OrderType, Side, SymbolId, TimeInForce, UnixMicros, UserId,
};
use exg_matching_engine::MatchingEngine;
use exg_protocol::Command;
use exg_risk_engine::SymbolConfig;

fn dec(s: &str) -> Decimal128 {
    s.parse().unwrap()
}

fn test_config() -> SymbolConfig {
    SymbolConfig {
        symbol: SymbolId::new(1),
        tick_size: dec("0.01"),
        lot_size: dec("0.001"),
        min_notional: dec("10"),
        max_leverage: dec("10"),
        maker_fee: dec("0.0002"),
        taker_fee: dec("0.0004"),
        margin_tiers: vec![],
    }
}

fn make_new_order(id: u64, user: u64, side: Side, price: &str, qty: &str) -> Command {
    Command::NewOrder {
        order_id: OrderId::new(id),
        user_id: UserId::new(user),
        symbol: SymbolId::new(1),
        side,
        order_type: OrderType::Limit,
        time_in_force: TimeInForce::Gtc,
        price: Some(dec(price)),
        quantity: dec(qty),
        stop_price: None,
        trailing_delta: None,
        visible_quantity: None,
        reduce_only: false,
        margin_mode: MarginMode::Cross,
        leverage: Some(dec("10")),
        client_order_id: None,
        timestamp: UnixMicros::from_micros(1_000_000 + id),
    }
}

/// Benchmark: single order match latency.
fn bench_single_match(c: &mut Criterion) {
    c.bench_function("single_order_match", |b| {
        b.iter_batched(
            || {
                let mut engine = MatchingEngine::new(test_config(), 1);
                // Place a resting ask
                let sell = make_new_order(1, 10, Side::Sell, "50000", "10");
                engine.process_command(&sell);
                engine
            },
            |mut engine| {
                let buy = make_new_order(2, 20, Side::Buy, "50000", "5");
                black_box(engine.process_command(&buy));
            },
            criterion::BatchSize::SmallInput,
        );
    });
}

/// Benchmark: insert 1000 orders then match all.
fn bench_insert_and_match_all(c: &mut Criterion) {
    c.bench_function("insert_1000_then_match_all", |b| {
        b.iter_batched(
            || {
                let mut engine = MatchingEngine::new(test_config(), 1);
                // Insert 1000 sell orders at various prices
                for i in 0..1000u64 {
                    let price = format!("{}", 50000 + i);
                    let sell = make_new_order(i + 1, i % 100 + 10, Side::Sell, &price, "1");
                    engine.process_command(&sell);
                }
                engine
            },
            |mut engine| {
                // One large market buy that sweeps through all 1000 orders
                let buy = Command::NewOrder {
                    order_id: OrderId::new(10_000),
                    user_id: UserId::new(1),
                    symbol: SymbolId::new(1),
                    side: Side::Buy,
                    order_type: OrderType::Market,
                    time_in_force: TimeInForce::Ioc,
                    price: None,
                    quantity: dec("1000"),
                    stop_price: None,
                    trailing_delta: None,
                    visible_quantity: None,
                    reduce_only: false,
                    margin_mode: MarginMode::Cross,
                    leverage: Some(dec("10")),
                    client_order_id: None,
                    timestamp: UnixMicros::from_micros(2_000_000),
                };
                black_box(engine.process_command(&buy));
            },
            criterion::BatchSize::SmallInput,
        );
    });
}

criterion_group!(
    benches,
    bench_single_match,
    bench_insert_and_match_all,
    bench_100k_orders
);
criterion_main!(benches);

fn bench_100k_orders(c: &mut Criterion) {
    let mut group = c.benchmark_group("throughput");
    group.sample_size(10);
    group.throughput(Throughput::Elements(100_000));

    group.bench_function("100k_orders_insert_match", |b| {
        b.iter_batched(
            || {
                let engine = MatchingEngine::new(test_config(), 1);
                // Pre-generate 100k order commands: alternating sell (resting) and buy (crossing)
                let mut cmds = Vec::with_capacity(100_000);
                for i in 0..100_000u64 {
                    if i % 2 == 0 {
                        // Sell orders at ascending prices
                        let price = format!("{}", 50000 + (i / 2));
                        cmds.push(make_new_order(i + 1, i % 100 + 10, Side::Sell, &price, "1"));
                    } else {
                        // Buy orders at prices that match resting sells
                        let price = format!("{}", 50000 + (i / 2));
                        cmds.push(make_new_order(i + 1, i % 100 + 200, Side::Buy, &price, "1"));
                    }
                }
                (engine, cmds)
            },
            |(mut engine, cmds)| {
                for cmd in &cmds {
                    black_box(engine.process_command(cmd));
                }
            },
            BatchSize::LargeInput,
        );
    });
    group.finish();
}
