use anyhow::anyhow;
use wackasm::{
    HaltError, HostYield, WackasmExecutor, WackasmModule,
    imports::{NoResources, WackasmHostBridge},
};
use wackasm_wasi::StdWasi;

use crate::wasi::CliImports;

pub mod wasi;

pub fn main() -> Result<(), anyhow::Error> {
    let contents = std::fs::read("./target/wasm32-wasip1/release/wasi.wasm")?;
    let module = WackasmModule::new(&contents)?;
    let sandbox = std::env::temp_dir().join(format!(
        "wackasm-wasi-{}-{}",
        std::process::id(),
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)?
            .as_nanos()
    ));
    std::fs::create_dir_all(&sandbox)?;
    let wasi = StdWasi::new(&sandbox)?
        .with_args(["wasi-test".to_owned(), "guest-arg".to_owned()])
        .with_environ(["WACKASM_WASI_TEST=enabled".to_owned()]);
    let mut host_bridge: WackasmHostBridge<_, _, NoResources> =
        WackasmHostBridge::new(wasi, CliImports {});
    let mut executor = WackasmExecutor::new(module)?;

    executor.call_export("_start", Vec::new())?;
    executor.set_fuel(isize::MAX);
    match executor.execute_with_imports(&mut host_bridge) {
        Ok(HostYield::Return) => println!("WASI test guest returned"),
        Err(HaltError::Trap(error)) if error.to_string() == "WASI process exited with status 0" => {
            println!("WASI test guest exited successfully")
        }
        Err(error) => return Err(error.into()),
        _ => return Err(anyhow!("WASI test guest yielded unexpectedly")),
    }
    drop(executor);
    drop(host_bridge);
    std::fs::remove_dir_all(sandbox)?;

    Ok(())
}
