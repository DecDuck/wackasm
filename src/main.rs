use std::time::Instant;

use wackasm::{Value, WackasmExecutor, WackasmModule};

pub fn main() -> Result<(), anyhow::Error> {
    let contents = std::fs::read("./examples/locals.wasm")?;
    let module = WackasmModule::new(&contents)?;
    let mut executor = WackasmExecutor::new(module);

    executor.call_export("sum_to_n", vec![Value::I32(22)])?;

    executor.set_fuel(3);
    executor.execute()?;

    println!("before serialize: {:?}", executor.get_result());

    let raw_executor = bitcode::serialize(&executor)?;
    drop(executor);
    println!("full suspended executor size: {}b", raw_executor.len());
    let mut executor: WackasmExecutor = bitcode::deserialize(&raw_executor)?;

    println!("after serialize: {:?}", executor.get_result());

    executor.set_fuel(100);
    executor.execute()?;

    println!("finish: {:?}", executor.get_result());

    Ok(())
}
