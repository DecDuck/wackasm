use std::hint::black_box;

use criterion::{BatchSize, Criterion, Throughput, criterion_group, criterion_main};
use wackasm::{Value, WackasmExecutor, WackasmModule};

const WASM_PATH: &str = "/home/decduck/wackasm/compute/target/wasm32-unknown-unknown/release/compute.wasm";

fn suspended_executor() -> anyhow::Result<Vec<u8>> {
    let contents = std::fs::read(WASM_PATH)?;
    let module = WackasmModule::new(&contents)?;
    let mut executor = WackasmExecutor::new(module)?;

    executor.call_export(
        "run_prime_benchmark",
        vec![Value::I32(100_000), Value::I32(80)],
    )?;
    executor.set_fuel(0);
    executor.execute()?;

    let serialized = bitcode::serialize(&executor)?;
    drop(executor);

    Ok(serialized)
}

fn execute(serialized: &[u8]) -> isize {
    let mut executor: WackasmExecutor =
        bitcode::deserialize(serialized).expect("deserialize suspended executor");
    executor.set_fuel(isize::MAX);
    executor.execute().expect("execute benchmark workload");

    black_box(isize::MAX - executor.get_fuel())
}

fn fuel_per_second(criterion: &mut Criterion) {
    let serialized = suspended_executor().expect("prepare suspended benchmark executor");
    let fuel_per_iteration = execute(&serialized) as u64;

    let mut group = criterion.benchmark_group("fuel_per_second");
    group.sample_size(10);
    group.throughput(Throughput::Elements(fuel_per_iteration));
    group.bench_function("run_prime_benchmark", |bencher| {
        bencher.iter_batched(
            || {
                bitcode::deserialize::<WackasmExecutor>(&serialized)
                    .expect("deserialize suspended executor")
            },
            |mut executor| {
                executor.set_fuel(isize::MAX);
                executor.execute().expect("execute benchmark workload");
                black_box(isize::MAX - executor.get_fuel());
            },
            BatchSize::SmallInput,
        );
    });
    group.finish();
}

criterion_group!(benches, fuel_per_second);
criterion_main!(benches);