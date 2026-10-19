use std::time::Instant;

use wackasm::{Value, WackasmExecutor, WackasmModule};

pub fn main() -> Result<(), anyhow::Error> {
    let contents = std::fs::read("./examples/table.wasm")?;
    let module = WackasmModule::new(&contents)?;
    let mut executor = WackasmExecutor::new(module);

    executor.call_export("apply", vec![Value::I32(1), Value::I32(4)])?;

    executor.set_fuel(3);
    executor.execute()?;

    println!("before serialize: {:?}", executor.get_result());

    let raw_executor = bitcode::serialize(&executor)?;
    drop(executor);
    println!("full suspended executor size: {}b", raw_executor.len());
    let mut executor: WackasmExecutor = bitcode::deserialize(&raw_executor)?;

    println!("after serialize: {:?}", executor.get_result());

    executor.set_fuel(1_000);
    executor.execute()?;

    println!("finish: {:?}", executor.get_result());

    Ok(())
}
