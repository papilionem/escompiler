use criterion::{Criterion, black_box, criterion_group, criterion_main};
use nanbox::JsValue;

fn bench_create_number(c: &mut Criterion) {
    c.bench_function("JsValue::number", |b| {
        b.iter(|| black_box(JsValue::number(2.5)))
    });
}

fn bench_create_int(c: &mut Criterion) {
    c.bench_function("JsValue::int", |b| b.iter(|| black_box(JsValue::int(42))));
}

fn bench_type_check(c: &mut Criterion) {
    let val = JsValue::number(2.5);
    c.bench_function("is_number", |b| b.iter(|| black_box(val.is_number())));
}

fn bench_extract(c: &mut Criterion) {
    let val = JsValue::int(42);
    c.bench_function("as_int", |b| b.iter(|| black_box(val.as_int())));
}

criterion_group!(
    benches,
    bench_create_number,
    bench_create_int,
    bench_type_check,
    bench_extract
);
criterion_main!(benches);
