use criterion::{Criterion, black_box, criterion_group, criterion_main};
use ir::builder::IrBuilder;
use ir::{Instruction, Type};

fn bench_build_add_function(c: &mut Criterion) {
    c.bench_function("build_add_fn", |b| {
        b.iter(|| {
            let mut builder = IrBuilder::new("add", vec![Type::Int32, Type::Int32], Type::Int32);
            let entry = builder.create_block();
            builder.switch_to_block(entry);
            let p0 = builder.push(Type::Int32, Instruction::Param(0));
            let p1 = builder.push(Type::Int32, Instruction::Param(1));
            let sum = builder.push(Type::Int32, Instruction::Add(p0, p1));
            builder.push(Type::Void, Instruction::Return(Some(sum)));
            black_box(builder.finish())
        })
    });
}

criterion_group!(benches, bench_build_add_function);
criterion_main!(benches);
