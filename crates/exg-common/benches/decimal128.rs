use criterion::{Criterion, black_box, criterion_group, criterion_main};
use exg_common::Decimal128;

fn bench_add(c: &mut Criterion) {
    let a: Decimal128 = "12345.6789".parse().unwrap();
    let b: Decimal128 = "98765.4321".parse().unwrap();
    c.bench_function("decimal128_add", |bencher| {
        bencher.iter(|| black_box(a) + black_box(b))
    });
}

fn bench_mul(c: &mut Criterion) {
    let a: Decimal128 = "12345.6789".parse().unwrap();
    let b: Decimal128 = "98765.4321".parse().unwrap();
    c.bench_function("decimal128_mul", |bencher| {
        bencher.iter(|| black_box(a) * black_box(b))
    });
}

fn bench_div(c: &mut Criterion) {
    let a: Decimal128 = "12345.6789".parse().unwrap();
    let b: Decimal128 = "98765.4321".parse().unwrap();
    c.bench_function("decimal128_div", |bencher| {
        bencher.iter(|| black_box(a) / black_box(b))
    });
}

fn bench_parse(c: &mut Criterion) {
    c.bench_function("decimal128_parse", |bencher| {
        bencher.iter(|| {
            black_box("12345.678901234567")
                .parse::<Decimal128>()
                .unwrap()
        })
    });
}

fn bench_display(c: &mut Criterion) {
    let v: Decimal128 = "12345.678901234567".parse().unwrap();
    c.bench_function("decimal128_display", |bencher| {
        bencher.iter(|| black_box(v).to_string())
    });
}

criterion_group!(
    benches,
    bench_add,
    bench_mul,
    bench_div,
    bench_parse,
    bench_display
);
criterion_main!(benches);
