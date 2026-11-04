use crate::{Value, WackasmExecutor, WackasmModule};

static WASM: &[u8] =
    include_bytes!("../../../../compute/target/wasm32-unknown-unknown/release/compute.wasm");

#[test]
pub fn suspension_test() {
    // baseline: run to completion
    let module = WackasmModule::new(WASM).expect("failed to parse wasm");
    let mut executor = WackasmExecutor::new(module).expect("failed to create executor");

    executor.set_fuel(isize::MAX);
    executor
        .call_export(
            "run_prime_benchmark",
            vec![Value::from_i32(1000000), Value::from_i32(80)],
        )
        .expect("failed to queue call_export");
    executor.execute().expect("failed to execute");
    let fuel_used = executor.get_fuel();

    let correct = executor.get_result().expect("no result");

    let module = WackasmModule::new(WASM).expect("failed to parse wasm");
    let mut executor = WackasmExecutor::new(module).expect("failed to create executor");

    executor
        .call_export(
            "run_prime_benchmark",
            vec![Value::from_i32(1000000), Value::from_i32(80)],
        )
        .expect("failed to queue call_export");

    let iterations = 5;
    let fuel_per_iterator = fuel_used / iterations;
    for _ in 0..iterations {
        executor.set_fuel(fuel_per_iterator);
        executor.execute().expect("failed to continue execution");
        let raw_executor = bitcode::serialize(&executor).expect("failed to suspend");
        drop(executor);
        executor = bitcode::deserialize(&raw_executor).expect("failed to resume");
    }

    let compare = executor.get_result().expect("failed to get result");
    assert_eq!(correct, compare);
}
