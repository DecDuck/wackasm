use std::{
    hint::black_box,
    time::{Duration, Instant},
};

use criterion::{
    BatchSize, Criterion, Throughput, criterion_group, criterion_main,
    measurement::{Measurement, ValueFormatter},
};
use wackasm::{Value, WackasmExecutor, WackasmModule};

static WASM: &[u8] = include_bytes!("../../../compute/target/wasm32-unknown-unknown/release/compute.wasm");

const LIMIT: u32 = 10_000;
const ROUNDS: u32 = 1000;

fn run_native() -> u64 {
    compute::run_prime_benchmark(black_box(LIMIT), black_box(ROUNDS))
}

/// Measures wall time in units of one native run of the same workload, so
/// results are comparable across CPUs with different clock speeds.
struct RelativeToNative {
    native_ns: f64,
}

impl RelativeToNative {
    fn calibrate() -> Self {
        let warm_up = Instant::now();
        while warm_up.elapsed() < Duration::from_secs(1) {
            black_box(run_native());
        }

        let mut samples = Vec::new();
        let sampling = Instant::now();
        while samples.len() < 50 || sampling.elapsed() < Duration::from_secs(2) {
            let start = Instant::now();
            black_box(run_native());
            samples.push(start.elapsed());
        }
        samples.sort_unstable();
        let median = samples[samples.len() / 2];
        println!("native baseline: {median:?} per run ({} samples)", samples.len());

        Self {
            native_ns: median.as_nanos() as f64,
        }
    }
}

impl Measurement for RelativeToNative {
    type Intermediate = Instant;
    type Value = f64;

    fn start(&self) -> Instant {
        Instant::now()
    }

    fn end(&self, start: Instant) -> f64 {
        start.elapsed().as_nanos() as f64 / self.native_ns
    }

    fn add(&self, a: &f64, b: &f64) -> f64 {
        a + b
    }

    fn zero(&self) -> f64 {
        0.0
    }

    fn to_f64(&self, value: &f64) -> f64 {
        *value
    }

    fn formatter(&self) -> &dyn ValueFormatter {
        self
    }
}

impl ValueFormatter for RelativeToNative {
    fn scale_values(&self, _typical_value: f64, _values: &mut [f64]) -> &'static str {
        "x native"
    }

    fn scale_throughputs(
        &self,
        _typical_value: f64,
        _throughput: &Throughput,
        values: &mut [f64],
    ) -> &'static str {
        for value in values {
            *value = 100.0 / *value;
        }
        "% of native"
    }

    fn scale_for_machines(&self, _values: &mut [f64]) -> &'static str {
        "native_runs"
    }
}

fn suspended_executor() -> anyhow::Result<Vec<u8>> {
    let module = WackasmModule::new(&WASM)?;
    let mut executor = WackasmExecutor::new(module)?;

    executor.call_export(
        "run_prime_benchmark",
        vec![Value(LIMIT as u64), Value(ROUNDS as u64)],
    )?;
    executor.set_fuel(0);
    executor.execute()?;

    let serialized = bitcode::serialize(&executor)?;
    drop(executor);

    Ok(serialized)
}

fn execute(serialized: &[u8]) -> (isize, u64) {
    let mut executor: WackasmExecutor =
        bitcode::deserialize(serialized).expect("deserialize suspended executor");
    executor.set_fuel(isize::MAX);
    executor.execute().expect("execute benchmark workload");

    let result = executor.get_result().expect("benchmark workload returned")[0].0;
    (isize::MAX - executor.get_fuel(), result)
}

fn percent_of_native(criterion: &mut Criterion<RelativeToNative>) {
    let serialized = suspended_executor().expect("prepare suspended benchmark executor");
    let (fuel_per_iteration, result) = execute(&serialized);
    assert_eq!(result, run_native(), "wackasm and native results differ");
    println!("wackasm: {fuel_per_iteration} fuel per run");

    let mut group = criterion.benchmark_group("percent_of_native");
    group.sample_size(50);
    // Only enables the "% of native" line; the formatter ignores the count.
    group.throughput(Throughput::Elements(1));
    group.bench_function("run_prime_benchmark", |bencher| {
        bencher.iter_batched(
            || {
                bitcode::deserialize::<WackasmExecutor>(&serialized)
                    .expect("deserialize suspended executor")
            },
            |executor| {
                let mut executor = black_box(executor);
                executor.set_fuel(isize::MAX);
                executor.execute().expect("execute benchmark workload");
                // Returned so criterion black-boxes it and drops it outside the timed region.
                executor
            },
            BatchSize::SmallInput,
        );
    });
    group.finish();
}

criterion_group! {
    name = benches;
    config = Criterion::default().with_measurement(RelativeToNative::calibrate());
    targets = percent_of_native
}
criterion_main!(benches);
