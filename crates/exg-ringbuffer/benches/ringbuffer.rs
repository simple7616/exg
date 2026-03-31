use criterion::{Criterion, Throughput, criterion_group, criterion_main};
use exg_ringbuffer::RingBuffer;

fn bench_push_pop_cycle(c: &mut Criterion) {
    let mut group = c.benchmark_group("ringbuffer");
    const MSG_COUNT: u64 = 1_000_000;
    const SLOT_COUNT: usize = 8192;
    const SLOT_SIZE: usize = 128;

    group.throughput(Throughput::Elements(MSG_COUNT));

    group.bench_function("push_pop_1M", |b| {
        b.iter(|| {
            let mut rb = RingBuffer::new(SLOT_COUNT, SLOT_SIZE).unwrap();
            let (producer, consumer) = rb.split();
            let payload = [0xABu8; 64];
            let mut buf = [0u8; 128];

            for _ in 0..MSG_COUNT {
                producer.try_push(&payload).unwrap();
                consumer.try_pop(&mut buf).unwrap();
            }
        });
    });

    group.finish();
}

criterion_group!(benches, bench_push_pop_cycle);
criterion_main!(benches);
