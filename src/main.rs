use std::time::Instant;

use wackasm::{Value, WackasmExecutor, WackasmModule};

pub fn main() -> Result<(), anyhow::Error> {
    let contents = std::fs::read("./compute/target/wasm32-unknown-unknown/release/compute.wasm")?;
    let module = WackasmModule::new(&contents)?;
    let mut executor = WackasmExecutor::new(module);

    executor.call_export("run_prime_benchmark", vec![Value::I32(1000000), Value::I32(80)])?;

    executor.set_fuel(10);
    executor.execute()?;

    println!("before serialize: {:?}", executor.get_result());

    let raw_executor = bitcode::serialize(&executor)?;
    drop(executor);
    println!("full suspended executor size: {}", human_bytes::human_bytes(raw_executor.len() as f64));
    let mut executor: WackasmExecutor = bitcode::deserialize(&raw_executor)?;

    println!("after serialize: {:?}", executor.get_result());

    executor.set_fuel(isize::MAX);
    let start = Instant::now();
    executor.execute()?;
    let time = start.elapsed();

    let consumed_fuel = isize::MAX - executor.get_fuel();

    let fpms = consumed_fuel as f64 / time.as_millis() as f64;
    let fps = fpms * 1000f64;

    println!("finish: {:?}", executor.get_result());
    println!("fps: {:.0}", fps);

    Ok(())
}
