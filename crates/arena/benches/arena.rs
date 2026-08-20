use arena::{Zone, ZoneId};
use criterion::{Criterion, black_box, criterion_group, criterion_main};

fn bench_alloc_u64(c: &mut Criterion) {
    c.bench_function("zone_alloc_u64", |b| {
        let mut zone = Zone::new(ZoneId(0));
        b.iter(|| {
            black_box(zone.alloc(42u64));
        });
    });
}

fn bench_alloc_then_reset(c: &mut Criterion) {
    c.bench_function("zone_alloc_1000_then_reset", |b| {
        let mut zone = Zone::new(ZoneId(0));
        b.iter(|| {
            for i in 0..1000u64 {
                black_box(zone.alloc(i));
            }
            zone.reset();
        });
    });
}

criterion_group!(benches, bench_alloc_u64, bench_alloc_then_reset);
criterion_main!(benches);
