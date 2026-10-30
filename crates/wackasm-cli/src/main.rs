use std::{fs::File, path::PathBuf, time::Instant};

use serde::{Deserialize, Serialize};
use wackasm::{
    Value, WackasmExecutor, WackasmModule,
    imports::{NoResources, WackasmResource},
};

pub mod wasi;

#[derive(Serialize, Deserialize)]
pub struct SuspendableFileHandle {
    pub id: Value,
    file_path: PathBuf,
    #[serde(skip)]
    file_handle: Option<File>,
}

#[derive(Serialize, Deserialize)]
pub enum CliResources {
    FileHandle(SuspendableFileHandle),
}

impl WackasmResource for CliResources {
    fn opaque_handle(&self) -> Value {
        match self {
            CliResources::FileHandle(suspendable_file_handle) => suspendable_file_handle.id,
        }
    }

    fn reactivate(self) -> Result<Self, anyhow::Error> {
        Ok(match self {
            CliResources::FileHandle(mut suspendable_file_handle) => {
                suspendable_file_handle.file_handle =
                    Some(File::open(suspendable_file_handle.file_path.clone())?);
                CliResources::FileHandle(suspendable_file_handle)
            }
        })
    }
}

pub fn main() -> Result<(), anyhow::Error> {
    let contents = std::fs::read("./compute/target/wasm32-unknown-unknown/release/compute.wasm")?;
    let module = WackasmModule::new(&contents)?;
    let mut executor: WackasmExecutor = WackasmExecutor::new(module)?;

    executor.call_export(
        "run_prime_benchmark",
        vec![Value::from_i32(1000000), Value::from_i32(80)],
    )?;

    executor.set_fuel(10);
    executor.execute()?;

    println!("before serialize: {:?}", executor.get_result());

    let raw_executor = bitcode::serialize(&executor)?;
    drop(executor);
    println!(
        "full suspended executor size: {}",
        human_bytes::human_bytes(raw_executor.len() as f64)
    );
    let mut executor: WackasmExecutor = bitcode::deserialize(&raw_executor)?;

    println!("after serialize: {:?}", executor.get_result());

    executor.set_fuel(isize::MAX);
    let start = Instant::now();
    executor.execute()?;
    let time = start.elapsed();

    let consumed_fuel = isize::MAX - executor.get_fuel();

    let time = time.as_millis() as f64;
    let fpms = consumed_fuel as f64 / time;
    let fps = fpms * 1000f64;

    println!("finish: {:?}", executor.get_result());
    println!("fps: {:.0}, total: {}s", fps, time / 1000.0f64);

    Ok(())
}
