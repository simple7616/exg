use criterion::{Criterion, criterion_group, criterion_main};
use std::path::PathBuf;

fn bench_sequential_writes(c: &mut Criterion) {
    c.bench_function("wal_sequential_write_1k_payload", |b| {
        let tmp = tempfile::TempDir::new().unwrap();
        let payload = vec![0xABu8; 1024];

        b.iter(|| {
            let config = exg_wal::WalConfig {
                dir: tmp.path().join("bench_run"),
                segment_size: 256 * 1024 * 1024,
                flush_interval_us: 1000,
                flush_every_n: 100_000,
            };
            let _ = std::fs::remove_dir_all(&config.dir);
            let mut w = exg_wal::WalWriter::open(config).unwrap();
            for _ in 0..10_000 {
                w.append(&payload).unwrap();
            }
            w.flush().unwrap();
        });
    });

    c.bench_function("wal_sequential_write_64b_payload", |b| {
        let tmp = tempfile::TempDir::new().unwrap();
        let payload = vec![0xCDu8; 64];

        b.iter(|| {
            let config = exg_wal::WalConfig {
                dir: tmp.path().join("bench_run_small"),
                segment_size: 256 * 1024 * 1024,
                flush_interval_us: 1000,
                flush_every_n: 100_000,
            };
            let _ = std::fs::remove_dir_all(&config.dir);
            let mut w = exg_wal::WalWriter::open(config).unwrap();
            for _ in 0..10_000 {
                w.append(&payload).unwrap();
            }
            w.flush().unwrap();
        });
    });
}

criterion_group!(benches, bench_sequential_writes);
criterion_main!(benches);
