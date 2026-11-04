use std::{
    hint::black_box,
    time::{Duration, Instant},
};

use criterion::{
    BatchSize, Criterion, Throughput, criterion_group, criterion_main,
    measurement::{Measurement, ValueFormatter},
};
use wackasm::{Value, WackasmExecutor, WackasmModule};

// Both wackasm and the wasmi baseline run exactly these bytes.
static WASM: &[u8] = include_bytes!("../../../compute/target/wasm32-unknown-unknown/release/compute.wasm");

const EXPORT: &str = "run_prime_benchmark";
const LIMIT: u32 = 1_000;
const ROUNDS: u32 = 1000;
const SAMPLES: usize = 500;

/// Runs `WASM` in wasmi, a mature interpreter, as the reference point.
struct WasmiBaseline {
    engine: wasmi::Engine,
    module: wasmi::Module,
    linker: wasmi::Linker<()>,
}

impl WasmiBaseline {
    fn new() -> Result<Self, wasmi::Error> {
        let mut config = wasmi::Config::default();
        config
            // Translate everything up front, like wackasm does in `WackasmModule::new`.
            .compilation_mode(wasmi::CompilationMode::Eager)
            // wackasm always meters fuel, so make wasmi pay for it too.
            .consume_fuel(true);
        let engine = wasmi::Engine::new(&config);
        let module = wasmi::Module::new(&engine, WASM)?;
        let linker = wasmi::Linker::new(&engine);
        Ok(Self {
            engine,
            module,
            linker,
        })
    }

    /// Instantiates a fresh copy (untimed) so every run starts from the same
    /// memory state as the restored wackasm executor, then times one call.
    fn run(&self) -> Result<(Duration, u64), wasmi::Error> {
        let mut store = wasmi::Store::new(&self.engine, ());
        store.set_fuel(u64::MAX)?;
        let instance = self.linker.instantiate_and_start(&mut store, &self.module)?;
        let function = instance.get_typed_func::<(u32, u32), u64>(&store, EXPORT)?;

        let start = Instant::now();
        let result = function.call(&mut store, (black_box(LIMIT), black_box(ROUNDS)))?;
        let elapsed = start.elapsed();

        Ok((elapsed, black_box(result)))
    }
}

/// Measures wall time in units of one wasmi run of the same module, so
/// results are comparable across CPUs with different clock speeds.
struct RelativeToWasmi {
    wasmi_ns: f64,
}

impl RelativeToWasmi {
    fn calibrate() -> Self {
        let baseline = WasmiBaseline::new().expect("compile wasm with wasmi");
        let run = || baseline.run().expect("run wasm with wasmi");

        let warm_up = Instant::now();
        while warm_up.elapsed() < Duration::from_secs(1) {
            run();
        }

        let mut samples = Vec::with_capacity(SAMPLES);
        while samples.len() < SAMPLES {
            samples.push(run().0);
        }
        samples.sort_unstable();
        let median = samples[samples.len() / 2];
        println!("wasmi baseline: {median:?} per run ({} samples)", samples.len());

        Self {
            wasmi_ns: median.as_nanos() as f64,
        }
    }
}

impl Measurement for RelativeToWasmi {
    type Intermediate = Instant;
    type Value = f64;

    fn start(&self) -> Instant {
        Instant::now()
    }

    fn end(&self, start: Instant) -> f64 {
        start.elapsed().as_nanos() as f64 / self.wasmi_ns
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

impl ValueFormatter for RelativeToWasmi {
    fn scale_values(&self, _typical_value: f64, _values: &mut [f64]) -> &'static str {
        "x wasmi"
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
        "% of wasmi"
    }

    fn scale_for_machines(&self, _values: &mut [f64]) -> &'static str {
        "wasmi_runs"
    }
}

fn suspended_executor() -> anyhow::Result<Vec<u8>> {
    let module = WackasmModule::new(&WASM)?;
    let mut executor = WackasmExecutor::new(module)?;

    executor.call_export(EXPORT, vec![Value(LIMIT as u64), Value(ROUNDS as u64)])?;
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

fn percent_of_wasmi(criterion: &mut Criterion<RelativeToWasmi>) {
    let serialized = suspended_executor().expect("prepare suspended benchmark executor");
    let (fuel_per_iteration, result) = execute(&serialized);
    let expected = WasmiBaseline::new()
        .and_then(|baseline| baseline.run())
        .expect("run wasm with wasmi")
        .1;
    assert_eq!(result, expected, "wackasm and wasmi results differ");
    println!("wackasm: {fuel_per_iteration} fuel per run");

    let mut group = criterion.benchmark_group("percent_of_wasmi");
    group.sample_size(SAMPLES);
    // Only enables the "% of wasmi" line; the formatter ignores the count.
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
    config = Criterion::default().with_measurement(RelativeToWasmi::calibrate());
    targets = percent_of_wasmi
}
criterion_main!(benches);
