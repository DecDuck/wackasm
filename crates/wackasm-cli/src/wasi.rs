use serde::{Deserialize, Serialize};
use wackasm::{
    HaltError, Value, WackasmExecutor, imports::{IntoWackasmCall as _, NoResources, WackasmImports, WackasmUntypedCall},
};

#[derive(Serialize, Deserialize, Clone)]
pub struct WasiHandler {}

fn println_i32(ctx: &mut WasiHandler, executor: &mut WackasmExecutor, address: u32) -> Result<(), HaltError> {
    let str = executor.memory().read_cstr(0, address).expect("failed to read memory");
    println!("{}", str.to_string_lossy());
    Ok(())
}

fn get_i32(ctx: &mut WasiHandler, executor: &mut WackasmExecutor,) -> Result<(i32,), HaltError> {
    todo!()
}

impl WackasmImports<WasiHandler, NoResources> for WasiHandler {
    fn get_import_func(
        &self,
        module: &str,
        name: &str,
    ) -> Option<Box<dyn WackasmUntypedCall<WasiHandler, NoResources>>> {
        match (module,name) {
            ("test", "print_i32") => Some(println_i32.into_call()),
            ("test", "get_i32") => Some(get_i32.into_call()),
            _ => None,
        }
    }
}
