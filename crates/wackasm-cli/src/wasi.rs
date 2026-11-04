use serde::{Deserialize, Serialize};
use wackasm::{
    HaltError, Value, imports::{IntoWackasmCall as _, NoResources, WackasmImports, WackasmUntypedCall},
};

#[derive(Serialize, Deserialize)]
pub struct WasiHandler {}

fn println_i32(ctx: &mut WasiHandler, first_arg: i32) {
    todo!()
}

fn get_i32(ctx: &mut WasiHandler,) -> (i32,) {
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
