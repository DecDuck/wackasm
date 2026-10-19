use std::{fs::File, time::Instant};

use wackasm::{Value, WackasmExecutor, WackasmModule};

pub fn main() -> Result<(), anyhow::Error> {
    let mut args = std::env::args().skip(1);
    let profile_output = match args.next() {
        None => None,
        Some(flag) if flag == "--profile" => {
            Some(args.next().unwrap_or_else(|| "flamegraph.svg".to_owned()))
        }
        Some(argument) => return Err(anyhow::anyhow!("unknown argument: {argument}")),
    };
    if args.next().is_some() {
        return Err(anyhow::anyhow!("usage: wackasm [--profile [output.svg]]"));
    }

    let contents = std::fs::read("./compute/target/wasm32-unknown-unknown/release/compute.wasm")?;
    let module = WackasmModule::new(&contents)?;
    let mut executor = WackasmExecutor::new(module);

    executor.call_export("run_prime_benchmark", vec![Value::I32(100000), Value::I32(80)])?;

    executor.set_fuel(0);
    executor.execute()?;

    println!("before serialize: {:?}", executor.get_result());

    let raw_executor = bitcode::serialize(&executor)?;
    drop(executor);
    println!("full suspended executor size: {}", human_bytes::human_bytes(raw_executor.len() as f64));
    let mut executor: WackasmExecutor = bitcode::deserialize(&raw_executor)?;

    println!("after serialize: {:?}", executor.get_result());

    executor.set_fuel(isize::MAX);
    let profiler = if profile_output.is_some() {
        Some(
            pprof::ProfilerGuardBuilder::default()
                .frequency(1000)
                .blocklist(&["libc", "libgcc", "pthread", "vdso"])
                .build()?,
        )
    } else {
        None
    };
    let start = Instant::now();
    executor.execute()?;
    let time = start.elapsed();

    if let (Some(profiler), Some(output_path)) = (profiler, profile_output) {
        let report = profiler.report().build()?;
        report.flamegraph(File::create(&output_path)?)?;
        println!("flamegraph: {output_path}");
    }

    let consumed_fuel = isize::MAX - executor.get_fuel();

    let fpms = consumed_fuel as f64 / time.as_millis() as f64;
    let fps = fpms * 1000f64;

    println!("finish: {:?}", executor.get_result());
    println!("fps: {:.0}", fps);

    Ok(())
}
