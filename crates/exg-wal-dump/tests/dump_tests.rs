use exg_common::{OrderId, SymbolId, UnixMicros, UserId};
use exg_protocol::{Event, RejectReason};
use exg_wal::{WalConfig, WalWriter};
use exg_wal_dump::dump;
use tempfile::TempDir;

fn ts() -> UnixMicros {
    UnixMicros::from_micros(1_700_000_000_000_000)
}

fn dec(s: &str) -> exg_common::Decimal128 {
    s.parse().unwrap()
}

fn wal_cfg(dir: &std::path::Path) -> WalConfig {
    WalConfig {
        dir: dir.to_path_buf(),
        segment_size: 64 * 1024 * 1024,
        flush_interval_us: 1000,
        flush_every_n: 1,
    }
}

fn write_events(dir: &std::path::Path, events: &[Event]) {
    let mut w = WalWriter::open(wal_cfg(dir)).unwrap();
    for e in events {
        let bytes = rkyv::to_bytes::<rkyv::rancor::Error>(e).unwrap();
        w.append(&bytes).unwrap();
    }
    w.flush().unwrap();
}

#[test]
fn happy_dump_three_events() {
    let tmp = TempDir::new().unwrap();
    let events = vec![
        Event::OrderAccepted {
            order_id: OrderId::new(1),
            user_id: UserId::new(42),
            symbol: SymbolId::new(1),
            client_order_id: None,
            timestamp: ts(),
        },
        Event::OrderRejected {
            order_id: OrderId::new(2),
            user_id: UserId::new(42),
            reason: RejectReason::InsufficientMargin,
            timestamp: ts(),
        },
        Event::OrderCanceled {
            order_id: OrderId::new(1),
            user_id: UserId::new(42),
            symbol: SymbolId::new(1),
            remaining_qty: dec("0.5"),
            timestamp: ts(),
        },
    ];
    write_events(tmp.path(), &events);

    let mut out = Vec::new();
    dump(tmp.path(), 0, &mut out).unwrap();
    let s = String::from_utf8(out).unwrap();
    let lines: Vec<&str> = s.lines().collect();
    assert_eq!(lines.len(), 3, "expected 3 JSON lines, got {}: {s}", lines.len());
    assert!(lines[0].contains("OrderAccepted"));
    assert!(lines[1].contains("OrderRejected"));
    assert!(lines[1].contains("InsufficientMargin"));
    assert!(lines[2].contains("OrderCanceled"));
}

#[test]
fn empty_wal_produces_no_output() {
    let tmp = TempDir::new().unwrap();
    let mut out = Vec::new();
    dump(tmp.path(), 0, &mut out).unwrap();
    assert!(out.is_empty(), "expected empty output, got {:?}", out);
}

#[test]
fn from_seq_filters_earlier_events() {
    let tmp = TempDir::new().unwrap();
    let events: Vec<Event> = (0..8)
        .map(|i| Event::OrderAccepted {
            order_id: OrderId::new(i),
            user_id: UserId::new(42),
            symbol: SymbolId::new(1),
            client_order_id: None,
            timestamp: ts(),
        })
        .collect();
    write_events(tmp.path(), &events);

    let mut out = Vec::new();
    dump(tmp.path(), 5, &mut out).unwrap();
    let s = String::from_utf8(out).unwrap();
    let lines: Vec<&str> = s.lines().collect();
    assert_eq!(lines.len(), 3, "from_seq=5 over 8 events should yield 3");
    assert!(lines[0].starts_with("5\t"), "first line: {}", lines[0]);
    assert!(lines[2].starts_with("7\t"), "last line: {}", lines[2]);
}

#[test]
fn corrupted_wal_returns_error() {
    let tmp = TempDir::new().unwrap();
    let event = Event::OrderAccepted {
        order_id: OrderId::new(1),
        user_id: UserId::new(42),
        symbol: SymbolId::new(1),
        client_order_id: None,
        timestamp: ts(),
    };
    write_events(tmp.path(), &[event]);

    let segments: Vec<_> = std::fs::read_dir(tmp.path())
        .unwrap()
        .filter_map(|e| e.ok())
        .filter(|e| e.file_name().to_string_lossy().starts_with("wal-"))
        .collect();
    assert!(!segments.is_empty());
    let seg_path = segments[0].path();
    let mut data = std::fs::read(&seg_path).unwrap();
    let mid = data.len() / 2;
    data[mid] ^= 0xFF;
    std::fs::write(&seg_path, data).unwrap();

    let mut out = Vec::new();
    let err = dump(tmp.path(), 0, &mut out);
    assert!(err.is_err(), "expected dump error for corrupt WAL");
}
