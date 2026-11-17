use serde::{Deserialize, Serialize};
use wackasm::{
    HaltError, WackasmExecutor,
    imports::{IntoWackasmCall as _, WackasmImports, WackasmResource, WackasmUntypedCall},
};
use wackasm_wasi::{WASI_PREVIEW1, WackasmWasiInterface, Wasi1};

#[derive(Serialize, Deserialize, Clone)]
pub struct CliImports {}

fn print_i32<Ctx: WackasmWasiInterface>(
    ctx: &mut Ctx,
    executor: &mut WackasmExecutor,
    address: u32,
) -> Result<(), HaltError> {
    let mut message = executor.memory().read_cstr(0, address)?.to_bytes().to_vec();
    message.push(b'\n');
    let mut written = 0;
    while written < message.len() {
        let count = ctx.fd_write(1, &message[written..]).map_err(|error| {
            HaltError::Error(anyhow::anyhow!(
                "WASI stdout write failed with errno {}",
                error.0
            ))
        })?;
        if count == 0 {
            return Err(HaltError::Error(anyhow::anyhow!(
                "WASI stdout write made no progress"
            )));
        }
        written += count;
    }
    Ok(())
}

impl<Ctx, R> WackasmImports<Ctx, R> for CliImports
where
    Ctx: WackasmWasiInterface + 'static,
    R: WackasmResource + 'static,
{
    fn get_import_func(
        &self,
        module: &str,
        name: &str,
    ) -> Option<Box<dyn WackasmUntypedCall<Ctx, R>>> {
        if module == WASI_PREVIEW1 {
            return <Wasi1 as WackasmImports<Ctx, R>>::get_import_func(&Wasi1, module, name);
        }

        match (module, name) {
            ("test", "print_i32") => Some(print_i32::<Ctx>.into_call()),
            _ => None,
        }
    }
}
